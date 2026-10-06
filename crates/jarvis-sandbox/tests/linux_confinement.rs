//! The Linux backend, asserted against a **live** kernel — the tests that can only run there.
//!
//! # These tests may never have run, and that is stated rather than hidden
//!
//! This crate is developed on Windows, so `cfg(target_os = "linux")` here means these tests are **type-checked**
//! by `cargo check --tests --target x86_64-unknown-linux-gnu` but executed only on Linux. A test that has only
//! been compiled is worth much less than one that has run, so the file exists to make the Linux behaviour
//! *runnable and reviewed* rather than to claim it is verified. `TODO.md` records the gap.
//!
//! Every test here **skips loudly** when the host has no delegated cgroup, printing what it could not check.
//! A silently skipped assertion is the failure mode the rest of this crate is written against, and a
//! `cfg`-based skip would be invisible in the test output.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use jarvis_sandbox::{
    Guarantee, Isolation, Limits, SandboxPolicy, SandboxRequest, Support, backend_for_host,
};

/// A shell command request, with an environment that lets a shell resolve its own builtins.
fn shell(script: &str) -> SandboxRequest {
    SandboxRequest {
        program: PathBuf::from("/bin/sh"),
        // A host process rather than a container, so no image: this suite exercises the cgroup backend, which
        // confines a pid rather than launching one from an image.
        image: None,
        arguments: vec!["-c".to_owned(), script.to_owned()],
        working_directory: None,
        // An allowlist, and a minimal one: a shell needs no inherited environment to run its builtins, and
        // handing it the daemon's `PATH` would be exactly the extension this crate prevents.
        environment: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        isolation: Isolation::Restricted,
        limits: Limits::default(),
        required: Vec::new(),
    }
}

/// Returns whether this host has a delegated cgroup, i.e. whether the backend this host chose **is the cgroup one**.
///
/// Not "whether the backend reports any support": a host with no delegated cgroup but a reachable Docker (every GitHub Actions
/// Ubuntu runner) is given the container backend, which has support too, and these tests are about the cgroup backend. They
/// must neither run against a container (which needs an image) nor treat it as delegation.
fn delegation_available() -> bool {
    backend_for_host().facility() == Support::CgroupV2
}

/// **A host that offers nothing refuses a required guarantee, and nothing is spawned.**
///
/// A Linux host with no delegated cgroup and no reachable container runtime is given the unconfined backend, whose support
/// set is empty. A request that *requires* a guarantee must then be refused by `SandboxPolicy::new`, **before** any launcher
/// exists: the alternative is a child that runs with none of the limits the caller was promised.
///
/// This replaces a test that expected the cgroup backend itself to refuse at launch. `backend_for_host()` no longer returns
/// a cgroup backend that cannot enforce anything (it falls back to a container, then to this), so that path is not reachable
/// through the public API; the refusal that *is* reachable is the one asserted here.
///
/// Falsified by mutation: letting the unconfined backend claim a guarantee fails the `unsupported` assertion.
#[test]
fn a_host_that_offers_nothing_refuses_a_required_guarantee() {
    let backend = backend_for_host();
    if backend.facility() != Support::Unconfined {
        eprintln!(
            "SKIPPED a_host_that_offers_nothing_refuses_a_required_guarantee: this host has a delegated cgroup or a \
             reachable container runtime, so it does offer something"
        );
        return;
    }
    let mut request = shell("true");
    request.required = vec![Guarantee::ProcessCountCeiling];
    request.limits.max_processes = Some(2);

    let error = SandboxPolicy::new(request, backend.as_ref())
        .err()
        .unwrap_or_else(|| panic!("a host that enforces nothing must refuse a required guarantee"));
    assert_eq!(error.unsupported(), Some(Guarantee::ProcessCountCeiling));
}
/// **A guarantee the Linux backend cannot enforce is refused by name — the platform asymmetry, on the platform.**
///
/// `CpuTimeCeiling` is the guarantee cgroup v2 genuinely lacks: it accounts CPU time in `cpu.stat` but has no
/// limit file for a cumulative total. A backend that reported it would turn a caller's "stop this child after N
/// seconds of CPU" into an indefinite throttle, which is a different promise.
///
/// The `backend.rs::tests` unit test asserts the same omission without needing Linux. This one asserts it
/// through the **public** API on the real host, so the two are not the same test: this one would also catch a
/// `probe()` that returned a support set assembled somewhere else.
#[tokio::test]
async fn a_cumulative_cpu_ceiling_is_refused_on_linux() {
    let backend = backend_for_host();
    if !delegation_available() {
        eprintln!(
            "SKIPPED a_cumulative_cpu_ceiling_is_refused_on_linux: no delegated cgroup, so the support set is \
             empty and the specific omission cannot be observed"
        );
        return;
    }
    let mut request = shell("true");
    request.required = vec![Guarantee::CpuTimeCeiling];
    let error = SandboxPolicy::new(request, backend.as_ref())
        .err()
        .unwrap_or_else(|| {
            panic!("cgroup v2 has no cumulative CPU limit, so this must be refused")
        });
    assert_eq!(error.unsupported(), Some(Guarantee::CpuTimeCeiling));
}

/// **A confined child is really moved into the launch's cgroup, which every guarantee depends on.**
///
/// The migration is the step that makes the limits apply: `pids.max` and `memory.max` are written before the
/// child exists, so the child is only bounded once it is **in** that cgroup. If the write silently addressed the
/// wrong pid, the launch would still report `Ok`, every limit would be inert, and nothing downstream could tell.
///
/// The evidence is `/proc/<pid>/cgroup`, read from the kernel rather than from our own bookkeeping: it is the
/// one place that says where the child actually is. Asserting our own recorded path instead would pass even if
/// the migration had gone somewhere else.
///
/// Falsified by mutation: writing a pid other than the child's fails here; skipping the `cgroup.procs` write
/// leaves the child in the parent's cgroup and fails here.
#[tokio::test]
async fn a_confined_child_is_really_moved_into_the_launch_cgroup() {
    if !delegation_available() {
        eprintln!(
            "SKIPPED a_confined_child_is_really_moved_into_the_launch_cgroup: no delegated cgroup on this host"
        );
        return;
    }
    let backend = backend_for_host();
    let request = shell("sleep 30");
    // `spawn`, not `launcher`: a name that close to `launched` trips `clippy::similar_names`, and the two are
    // genuinely easy to confuse in a test that reasons about both.
    let spawn = jarvis_sandbox::stdio_launcher(false);
    let policy = SandboxPolicy::new(request, backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));

    let launched = backend
        .launch(&policy, spawn)
        .await
        .unwrap_or_else(|error| {
            panic!("a confined launch on a delegated host must succeed: {error}")
        });
    let pid = launched.process.pid();
    let membership = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .unwrap_or_else(|error| panic!("the kernel must report pid {pid}'s cgroup: {error}"));

    assert!(
        membership.contains("jarvis-sandbox"),
        "the child is not in the sandbox cgroup, so every limit written into it is inert; kernel says: {membership}"
    );
    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing the confined child must succeed: {error}"));
}

/// **A process-count ceiling is enforced by the kernel, so a child that forks past it fails.**
///
/// `pids.max` is the guarantee that cannot be approximated on Linux: `RLIMIT_NPROC` is counted **per user id**,
/// so a child that reached its own limit could still fork. The only honest test is to make a real child try to
/// fork past a real limit and observe that the kernel refuses — which is what this does, bounded so that a
/// misconfigured run cannot become the fork bomb it is testing for.
///
/// Falsified by mutation: dropping the `pids.max` write lets the shell create more processes than the ceiling and
/// fails here; writing the limit to the wrong file fails here.
#[tokio::test]
async fn a_process_count_ceiling_is_enforced_by_the_kernel() {
    if !delegation_available() {
        eprintln!(
            "SKIPPED a_process_count_ceiling_is_enforced_by_the_kernel: no delegated cgroup on this host"
        );
        return;
    }
    let backend = backend_for_host();
    let mut request = shell(
        // Bounded on purpose: at most 12 attempts, each a short sleep so the shell cannot busy-loop, and the
        // attempts themselves are what the ceiling must refuse. This can never become an unbounded fork bomb.
        "i=0; ok=0; while [ $i -lt 12 ]; do if sleep 10 & then ok=$((ok+1)); else break; fi; i=$((i+1)); done; \
         echo FORKED=$ok; wait",
    );
    request.required = vec![Guarantee::ProcessCountCeiling];
    // Two tasks: the shell itself plus exactly one background sleep. Anything beyond that must fail.
    request.limits.max_processes = Some(2);

    let spawn = jarvis_sandbox::stdio_launcher(true);
    let policy = SandboxPolicy::new(request, backend.as_ref())
        .unwrap_or_else(|error| panic!("a cgroup host must support a process ceiling: {error}"));
    let mut launched = backend
        .launch(&policy, spawn)
        .await
        .unwrap_or_else(|error| panic!("a confined launch must succeed: {error}"));

    let mut stdout = launched
        .stdout
        .take()
        .unwrap_or_else(|| panic!("piped stdout was requested"));
    // Bounded: if the ceiling did NOT apply, the shell would loop for 12 x 10s. The timeout is the assertion's
    // failure mode — a run that hangs is reported as "the ceiling did not apply" rather than hanging the suite.
    let mut buffer = vec![0_u8; 4096];
    let read = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::io::AsyncReadExt::read(&mut stdout, &mut buffer),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the child did not finish within 8s, so the process ceiling did not stop it from forking repeatedly"
        )
    })
    .unwrap_or_else(|error| panic!("reading the child's output must succeed: {error}"));
    let reported = String::from_utf8_lossy(&buffer[..read]).to_string();
    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing the confined child must succeed: {error}"));

    // `FORKED=1` is the ceiling working: the shell itself and one background child fit, the rest were refused.
    assert!(
        reported.contains("FORKED=1"),
        "a ceiling of two tasks must let exactly one `sleep` start, got: {reported}"
    );
}
