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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jarvis_sandbox::{
    Guarantee, Isolation, Limits, SandboxError, SandboxPolicy, SandboxRequest, backend_for_host,
    refusing_launcher,
};

/// A shell command request, with an environment that lets a shell resolve its own builtins.
fn shell(script: &str) -> SandboxRequest {
    SandboxRequest {
        program: PathBuf::from("/bin/sh"),
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

/// Returns whether this host has a delegated cgroup, i.e. whether the backend reports any support.
fn delegation_available() -> bool {
    !backend_for_host().support().is_empty()
}

/// **A host with no delegation refuses, and the refusal happens without spawning anything.**
///
/// This is the ordering property the whole design rests on: the backend writes its limits **before** a child
/// exists, so a refusal at that point must not have created one. The `refusing_launcher` records whether it was
/// called, which is exact — a process-table inspection would be racy, and would fail only when a race was lost.
///
/// It also asserts the refusal is a `Setup` failure naming the facility, because the two error kinds call for
/// different operator actions: `Setup` means "delegate a cgroup on this host", `Launch` means "the program could
/// not be started".
///
/// Falsified by mutation: calling `launcher()` before the delegation check fails the `observed` assertion;
/// mapping the missing delegation to `SandboxError::Launch` fails the kind assertion.
#[tokio::test]
async fn a_host_without_a_delegation_refuses_without_spawning() {
    if delegation_available() {
        eprintln!(
            "SKIPPED a_host_without_a_delegation_refuses_without_spawning: this host HAS a delegated cgroup, so \
             the no-delegation path cannot be reached here"
        );
        return;
    }
    let backend = backend_for_host();
    let observed = Arc::new(AtomicBool::new(false));
    // No requirement, so `SandboxPolicy::new` accepts and the refusal comes from `launch`. That is the path
    // being tested: a *permitted* policy that the facility still cannot honour.
    let policy = SandboxPolicy::new(shell("true"), backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));

    let outcome = backend
        .launch(&policy, refusing_launcher(Arc::clone(&observed)))
        .await;

    assert!(
        !observed.load(Ordering::SeqCst),
        "the launcher was called, so a child was created before the delegation was checked"
    );
    match outcome {
        Err(SandboxError::Setup { facility, .. }) => {
            assert_eq!(
                facility, "cgroup v2",
                "the refusal must name the facility an operator has to fix"
            );
        }
        Err(other) => panic!("a missing delegation is a setup failure, got: {other}"),
        Ok(_) => panic!("a host with no delegation cannot have launched a confined child"),
    }
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
    let spawn = jarvis_sandbox::launcher_for(&request, false);
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

    let spawn = jarvis_sandbox::launcher_for(&request, true);
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
