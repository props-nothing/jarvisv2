//! The container backend, asserted against a **live runtime** — the acceptance proof for `P3-020`.
//!
//! # Why the acceptance test is a real launch and not an argv assertion
//!
//! The unit tests assert the argv exactly, which is the right way to pin the flags. But an argv that *says*
//! `--read-only` and `--network none` is a claim about a string; the property `P3-020` exists to establish is
//! that a container **actually refuses a write**, and only running one can show that. So this file launches real
//! containers with a real image and reads real exit statuses.
//!
//! Writing it also surfaced a defect in the crate rather than in the test: `LaunchedProcess` had `pid()` and
//! `kill()` and **no way to wait**, so no live property of any backend could be observed at all — a handle that
//! could be destroyed but never read. `wait` was added for this file, and the limitation it carries (a consuming
//! wait cannot be cancelled) is recorded on the trait rather than hidden here.
//!
//! # And why it skips loudly when there is no runtime
//!
//! A container runtime is a host facility, so on a host without one these tests have nothing to check — and a
//! silently skipped assertion is the failure mode the rest of this crate is written against. Every test therefore
//! prints what it could not check and why, and the image is one an operator has to have **already** placed,
//! because the backend never pulls (pulling is a network fetch of content nobody chose).
//!
//! The default image is `alpine:3` because it is small and ubiquitous. `JARVIS_SANDBOX_TEST_IMAGE` overrides it
//! for a host whose local images differ, which is what lets these tests run in an environment that has, say, a
//! pinned `alpine:3.20` and no moving tag.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jarvis_sandbox::{
    ContainerBackend, Guarantee, Isolation, Launched, Limits, SandboxBackend, SandboxError,
    SandboxPolicy, SandboxRequest, refusing_launcher, stdio_launcher,
};

/// The image these tests run, overridable because the backend deliberately never pulls one.
///
/// A moving tag is a fixture whose contents change under the suite, which is normally worth avoiding — here it
/// is accepted because the assertions are about **confinement** rather than about program output, and they hold
/// for any Alpine.
fn test_image() -> String {
    std::env::var("JARVIS_SANDBOX_TEST_IMAGE").unwrap_or_else(|_| "alpine:3".to_owned())
}

/// Serialises the live container tests.
///
/// # Why these tests cannot run in parallel, and why an in-process lock is the honest fix
///
/// A container runtime keeps **one** list of containers for the whole host, so it is shared state that a test
/// cannot mark as its own. The first version of these tests ran in parallel and failed in two different ways:
///
/// - A leftover container from another test appeared in this one's `docker ps`, so a leaked container and a
///   concurrent test's container were indistinguishable — and the failure message named a pid that was not this
///   process's, which is how the interference was identified rather than assumed.
/// - The reverse also happened: a container created by a test that was still starting had not appeared yet when
///   another test listed containers, and an absence was read as a removal that had not been attempted.
///
/// Both are fixed by running these tests one at a time, which is what the lock does. Every live test takes it, so
/// the guarantee is about the whole suite rather than about one assertion.
///
/// A **Tokio** mutex rather than a `std` one, because the guard has to be held across an `await` — the whole
/// point is to keep the runtime's list to oneself while a launch is in flight — and `std::sync::MutexGuard`
/// held across an await is both a lint and a real hazard on a multi-threaded runtime.
static RUNTIME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Takes the runtime lock, returning a guard that must be held for the test's duration.
async fn exclusive() -> tokio::sync::MutexGuard<'static, ()> {
    RUNTIME_LOCK.lock().await
}

/// Returns the probe, printing what could not be checked when no runtime is reachable.
///
/// Returning `Option` and printing makes the skip **loud** in the test output, per the convention the Linux
/// suite established. A `#[cfg]`-based skip would be invisible, and `#[ignore]` would make the tests look unrun
/// on a host where they could have run.
fn backend() -> Option<ContainerBackend> {
    let backend = ContainerBackend::probe();
    if backend.support().is_empty() {
        eprintln!(
            "jarvis-sandbox: no reachable container runtime, so the container acceptance tests have nothing to \
             check on this host"
        );
        return None;
    }
    Some(backend)
}

/// Returns the image these tests need, or `None` after printing why it is absent.
///
/// Asked **before** a launch, so a missing fixture is reported as a fixture problem rather than surfacing as a
/// sandbox refusal. The distinction matters: the backend's `--pull never` behaviour is correct, and a test that
/// hit it would fail for a reason having nothing to do with confinement.
fn image() -> Option<String> {
    let image = test_image();
    let present = std::process::Command::new("docker")
        .args(["image", "inspect", &image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !present {
        eprintln!(
            "jarvis-sandbox: the image `{image}` is not present locally and this backend never pulls one, so \
             the container acceptance tests have nothing to check. Place it with `docker pull {image}`."
        );
        return None;
    }
    Some(image)
}

/// A container request running one command inside the test image.
fn request(image: &str, command: &str, arguments: Vec<String>) -> SandboxRequest {
    SandboxRequest {
        program: PathBuf::from(command),
        image: Some(image.to_owned()),
        arguments,
        working_directory: None,
        // Empty, deliberately: a shell needs the image's own environment to resolve a helper, and the
        // environment boundary is asserted on its own in `the_environment_boundary_replaces_the_images_own`.
        environment: BTreeMap::new(),
        isolation: Isolation::Restricted,
        limits: Limits::default(),
        required: Vec::new(),
    }
}

/// Launches a policy and returns the launch, or panics with the error.
///
/// `piped` is a parameter because it is the one thing the request cannot state: whether the caller intends to
/// drive the child or only to supervise it.
async fn launch(backend: &ContainerBackend, request: SandboxRequest, piped: bool) -> Launched {
    let policy = SandboxPolicy::new(request, backend)
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));
    let launched = backend
        .launch(&policy, stdio_launcher(piped))
        .await
        .unwrap_or_else(|error| panic!("a container launch must succeed: {error}"));
    assert_eq!(
        launched.support.as_str(),
        "container",
        "the launch must report the facility that confined it"
    );
    launched
}

/// Waits for a launch and returns its status with everything it captured.
///
/// The order is deliberate: the pipes are taken **before** the wait, because a consuming wait takes the value
/// that owns them. Draining afterwards is sound only for fixtures whose output is far below a pipe buffer — a
/// child writing more than the buffer while the parent waited would deadlock. That is a real limitation of this
/// helper, which is why it is used only where the output is known to be small.
async fn finish(mut launched: Launched) -> (std::process::ExitStatus, String) {
    let stdout = launched.stdout.take();
    let stderr = launched.stderr.take();
    let status = launched
        .process
        .wait()
        .await
        .unwrap_or_else(|error| panic!("collecting the child's status must succeed: {error}"));
    let mut output = String::new();
    if let Some(reader) = stdout {
        output.push_str(&drain(reader).await);
    }
    if let Some(reader) = stderr {
        output.push_str(&drain(reader).await);
    }
    (status, output)
}

/// Drains a child's stream into a string.
///
/// One generic helper rather than one per stream: `ChildStdout` and `ChildStderr` differ only in their name, and
/// two copies of this would be two places for the same `read_to_string` to be wrong.
async fn drain<R>(mut reader: R) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;
    let mut buffer = String::new();
    reader
        .read_to_string(&mut buffer)
        .await
        .unwrap_or_else(|error| panic!("reading the child's stream failed: {error}"));
    buffer
}

/// **A read-only root filesystem actually refuses a write — observed, not inferred from a flag.**
///
/// This is the acceptance property for `P3-020`. Passing `--read-only` is a claim about an argv; the claim worth
/// making is that a container cannot write, and the difference between the two is why this launches a real
/// container. Both `/` and `/tmp` are attempted, so an implementation that mounted a writable `/tmp` — a
/// plausible "fix" for programs needing scratch space — is caught too.
///
/// The **message** is asserted rather than only a non-zero status, because a non-zero status is also produced by
/// a command that failed for an unrelated reason: a missing program, an image whose `sh` is elsewhere, or a
/// container that never started. `Read-only file system` is the kernel's refusal, so reading it is what makes
/// this a test of the confinement rather than of the fixture.
///
/// Falsified by mutation: removing `--read-only` from `container_arguments` makes both writes succeed, and the
/// status and message assertions fail together.
#[tokio::test]
async fn a_read_only_container_refuses_a_write() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    // `sh -c` rather than an argv vector, because the script needs `;` and `2>&1`. That is a decision about this
    // fixture; the backend never builds a shell string, and the element-boundary property is asserted separately
    // by `arguments_reach_the_program_with_their_boundaries_intact`.
    let request = request(
        &image,
        "/bin/sh",
        vec![
            "-c".to_owned(),
            "touch /x 2>&1; touch /tmp/y 2>&1".to_owned(),
        ],
    );
    let (status, output) = finish(launch(&backend, request, true).await).await;
    assert!(
        output.contains("Read-only file system"),
        "the kernel's refusal must be what the container reports, got: {output:?}"
    );
    assert!(
        !status.success(),
        "a failed write must reach the caller as a failed status, got {status:?}"
    );
}

/// **The container's exit status reaches the caller — so a launch can be supervised.**
///
/// The `docker` CLI **is** the process under confinement: `run` waits for the container and propagates its exit
/// code (probed: `exit 7` → shell `7`). A caller that could not observe the status would have a launch it cannot
/// supervise, which is why this is an acceptance property and not a detail. It is asserted for a success **and**
/// a failure, because an implementation that always reported failure would satisfy the second alone.
///
/// The specific codes **7** and **0** are asserted rather than `is_success`, because propagation is the property:
/// an implementation that reported a constant `1` for every non-zero container exit would pass a
/// success/non-success check while losing the information a supervisor needs.
///
/// Falsified by mutation: launching with `--detach` makes the CLI return before the container has run, so it
/// reports its own success and the `exit 7` case fails.
#[tokio::test]
async fn the_containers_exit_status_reaches_the_caller() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    for (script, expected) in [("exit 0", 0), ("exit 7", 7)] {
        let request = request(&image, "/bin/sh", vec!["-c".to_owned(), script.to_owned()]);
        let (status, output) = finish(launch(&backend, request, true).await).await;
        assert_eq!(
            status.code(),
            Some(expected),
            "`{script}` must reach the caller as {expected}, got {status:?} with output {output:?}"
        );
    }
}

/// **Killing a launch removes the container, rather than only the CLI that started it.**
///
/// The failure this exists to prevent is specific and silent: `Child::kill` on the `docker` CLI leaves the
/// **container** running, because the CLI is a client and the work happens in the daemon. So the assertion is
/// not "the CLI died" — it is that the container is gone from the **runtime's own view**, which is why the check
/// runs `docker ps` instead of inspecting a process table.
///
/// Falsified by mutation: implementing `kill` as `child.kill()` alone leaves the container listed as `Up`, and
/// the second assertion fails.
#[tokio::test]
async fn killing_a_launch_removes_the_container() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    // A command that outlives the check, so "is it still running" is a meaningful question rather than a race
    // against a container that has already finished.
    let request = request(
        &image,
        "/bin/sh",
        vec!["-c".to_owned(), "sleep 120".to_owned()],
    );
    let launched = launch(&backend, request, false).await;
    assert_ne!(
        launched.process.pid(),
        0,
        "a live launch must report the CLI's pid for correlation"
    );
    let ours = wait_for_owned_containers().await;
    assert_eq!(
        ours.len(),
        1,
        "the launch must have created exactly one named container, found {ours:?} among {:?}",
        running_containers()
    );
    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing a launch must succeed: {error}"));
    // Asked again from the runtime, so the assertion is about the state it is in rather than about a value this
    // test happened to construct.
    let after = running_containers();
    assert!(
        !after.iter().any(|name| ours.contains(name)),
        "the container must be gone after a kill, not merely its client: still present in {after:?}"
    );
}

/// **The environment boundary replaces the image's own environment.**
///
/// Probed before it was implemented, and it is the finding that changed the design: `-e FOO=bar` **adds** to an
/// image's environment (`PATH` and `HOME` survive), while `SandboxRequest::environment` is documented as the
/// **complete** environment and the process backend implements it as `env_clear().envs(…)`. So the same request
/// would mean two different variable sets depending on which backend served it.
///
/// The assertions are about variables the **image** supplies, which is what distinguishes a boundary from an
/// addition: `PATH` and `HOME` are in Alpine's own environment, so seeing `JARVIS_PROBE` without them proves the
/// environment was cleared, where checking only for `JARVIS_PROBE` would pass for an implementation using `-e`.
///
/// Falsified by mutation: replacing `env -i` with plain `-e` flags leaves `PATH` in the output and this fails.
#[tokio::test]
async fn the_environment_boundary_replaces_the_images_own() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    let mut request = request(&image, "/usr/bin/env", Vec::new());
    request.environment = BTreeMap::from([("JARVIS_PROBE".to_owned(), "stated".to_owned())]);
    let (_, output) = finish(launch(&backend, request, true).await).await;
    let lines: Vec<&str> = output.lines().collect();
    assert!(
        lines.contains(&"JARVIS_PROBE=stated"),
        "the request's own variable must reach the container, got: {output:?}"
    );
    for image_variable in ["PATH=", "HOME=", "HOSTNAME="] {
        assert!(
            !lines.iter().any(|line| line.starts_with(image_variable)),
            "{image_variable} comes from the image rather than the request, so it must be cleared: {output:?}"
        );
    }
}

/// **An empty stated environment leaves the image's own, and that asymmetry is deliberate.**
///
/// The control for the test above, and the reason it is a separate assertion: a request whose `environment` is
/// empty passes no `env` at all and runs the program directly, so the image's `PATH` survives. An implementation
/// that wrapped **every** launch in `env -i` would satisfy the boundary test while breaking any image whose
/// program resolves a helper by name — a real behavioural difference rather than an optimisation.
///
/// An empty map reads as "I stated no variables", and the honest interpretation of that is the image's default
/// rather than "remove everything". The asymmetry is pinned here so it cannot drift silently.
///
/// Falsified by mutation: wrapping every launch in `env -i` removes `PATH` and this fails.
#[tokio::test]
async fn an_empty_stated_environment_leaves_the_images_own() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    let request = request(&image, "/usr/bin/env", Vec::new());
    assert!(
        request.environment.is_empty(),
        "this test is about the empty case and must be built with one"
    );
    let (_, output) = finish(launch(&backend, request, true).await).await;
    assert!(
        output.lines().any(|line| line.starts_with("PATH=")),
        "a request that states no environment must leave the image's own in place, got: {output:?}"
    );
}

/// **A memory ceiling the runtime will not accept is refused before any container exists.**
///
/// Asserted through a **live backend**, not a stub, so the property is that a real launch path refuses rather
/// than that a pure function does. `refusing_launcher` records whether a launcher was called, which is the exact
/// form of "no container was started" — a process-table check would be racy, and would fail only when a race was
/// lost.
///
/// Falsified by mutation: dropping the floor check turns this into a failure inside the runtime, which is a
/// different error kind with a different operator action.
#[tokio::test]
async fn an_unacceptable_memory_ceiling_is_refused_without_starting_a_container() {
    let _guard = exclusive().await;
    let Some(backend) = backend() else {
        return;
    };
    let mut request = request("alpine:3", "/bin/true", Vec::new());
    request.limits = Limits {
        max_memory_bytes: Some(1),
        ..Limits::default()
    };
    // The guarantee is required, so this reaches the launch only because the *backend* claims it and the *value*
    // is what cannot be expressed — the distinction between a policy check and a launch refusal.
    request.required = vec![Guarantee::MemoryCeiling];
    let policy = SandboxPolicy::new(request, &backend).unwrap_or_else(|error| {
        panic!("the backend claims MemoryCeiling, so this is accepted here: {error}")
    });
    let observed = Arc::new(AtomicBool::new(false));
    let error = backend
        .launch(&policy, refusing_launcher(observed.clone()))
        .await
        .err()
        .unwrap_or_else(|| {
            panic!("a 1-byte memory ceiling cannot be expressed and must be refused")
        });
    assert!(
        matches!(error, SandboxError::Launch { .. }),
        "the facility is fine and the request is not, so this is a launch refusal, got: {error:?}"
    );
    assert!(
        error.to_string().contains("6291456"),
        "the refusal must name the floor in bytes so an operator can act, got: {error}"
    );
    assert!(
        !observed.load(Ordering::SeqCst),
        "no container may be started for a request the backend is about to refuse"
    );
}

/// **A container request with no image is refused by name.**
///
/// `image` is optional because the requirement belongs to one backend — a cgroup request has no use for one — so
/// the refusal lives at the launch rather than in `SandboxPolicy::new`. What must not happen is a fallback to
/// `program`: a host path and an image reference are different kinds of name, and accepting either would make
/// them interchangeable, so a caller who meant "run this binary" could silently get a different one from an
/// image, or the reverse.
///
/// Falsified by mutation: deriving the image from `program` makes the launch proceed and this fails.
#[tokio::test]
async fn a_container_request_without_an_image_is_refused_by_name() {
    let _guard = exclusive().await;
    let Some(backend) = backend() else {
        return;
    };
    let mut request = request("alpine:3", "/bin/sh", Vec::new());
    request.image = None;
    let policy = SandboxPolicy::new(request, &backend).unwrap_or_else(|error| {
        panic!("an image is not a guarantee, so this is accepted here: {error}")
    });
    let observed = Arc::new(AtomicBool::new(false));
    let error = backend
        .launch(&policy, refusing_launcher(observed.clone()))
        .await
        .err()
        .unwrap_or_else(|| panic!("a container launch without an image must be refused"));
    assert!(
        error.to_string().contains("image"),
        "the refusal must name the missing field, got: {error}"
    );
    assert!(
        !observed.load(Ordering::SeqCst),
        "no container may be started for a request the backend refuses"
    );
}

/// **Arguments reach the program with their element boundaries intact.**
///
/// `docs/architecture/security.md` asks for "structured commands where possible; no host shell by default", and
/// a container's argv is where a shell would be easiest to reintroduce — by joining the command into a string
/// for `docker run`. So `printf` is asked to show its arguments, and values containing a space and a shell
/// metacharacter must arrive **unmodified and separate**.
///
/// Falsified by mutation: building the command as one shell string splits `a b` into two arguments and expands
/// `$d`, and this fails.
#[tokio::test]
async fn arguments_reach_the_program_with_their_boundaries_intact() {
    let _guard = exclusive().await;
    let (Some(backend), Some(image)) = (backend(), image()) else {
        return;
    };
    // A bracketed format, so the output shows exactly how many arguments arrived and where each begins. `$d` is
    // a literal here: a shell that saw it would expand it to nothing.
    let request = request(
        &image,
        "/usr/bin/printf",
        vec!["[%s]".to_owned(), "a b".to_owned(), "c$d".to_owned()],
    );
    let (status, output) = finish(launch(&backend, request, true).await).await;
    assert!(
        status.success(),
        "the program must have run, got {status:?} with output {output:?}"
    );
    assert_eq!(
        output.trim(),
        "[a b][c$d]",
        "both arguments must arrive unmodified and separate, got: {output:?}"
    );
}

/// Lists the names of every container the runtime knows about.
fn running_containers() -> Vec<String> {
    let output = std::process::Command::new("docker")
        .args(["ps", "-a", "--format", "{{.Names}}"])
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        Ok(output) => panic!(
            "`docker ps` failed with {}, so the container state cannot be read",
            output.status
        ),
        Err(error) => panic!("`docker ps` could not be run: {}", error.kind()),
    }
}

/// Lists the JARVIS-owned containers, waiting for at least one to appear.
///
/// # Why waiting is required rather than a sleep
///
/// `spawn` returns when the **client** exists, and the container is created by the daemon afterwards, so a list
/// taken immediately can legitimately be empty. Polling until the container exists is the difference between a
/// test of the backend and a test of the host's scheduling: the first version asserted immediately and failed on
/// a launch that had in fact worked.
///
/// The bound is a failure rather than a skip, because the lock is held and no other test can be the reason a named
/// container did not appear — under the lock, an absence is the backend's answer.
async fn wait_for_owned_containers() -> Vec<String> {
    for _ in 0..50 {
        let owned = owned_containers();
        if !owned.is_empty() {
            return owned;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "a launch must create a named container within 5 seconds; none appeared among {:?}",
        running_containers()
    );
}

/// Lists the containers whose names this crate generates.
fn owned_containers() -> Vec<String> {
    running_containers()
        .into_iter()
        .filter(|name| name.starts_with("jarvis-sandbox-"))
        .collect()
}
