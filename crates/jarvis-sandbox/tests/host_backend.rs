//! What this host's backend can enforce, asserted against what the platform documents.
//!
//! These tests run on **every** platform, including ones with no backend, because the interesting property is
//! not "a sandbox works here" — it is that the backend never *claims* more than the host can do. A crate that
//! reported `ProcessCountCeiling` on a host with no cgroup would pass every confinement test written for Linux
//! and fail exactly the operator it was meant to protect.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use jarvis_sandbox::{
    Guarantee, Isolation, Limits, SandboxError, SandboxPolicy, SandboxRequest, backend_for_host,
    refusing_launcher,
};

/// A request that requires nothing, so `SandboxPolicy::new` accepts it on any host.
fn unconfined_request(program: PathBuf, arguments: Vec<String>) -> SandboxRequest {
    SandboxRequest {
        program,
        arguments,
        working_directory: None,
        environment: BTreeMap::new(),
        isolation: Isolation::None,
        limits: Limits::default(),
        required: Vec::new(),
    }
}

/// A program that prints its environment and exits, for asserting the allowlist.
fn env_dump() -> (PathBuf, Vec<String>) {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        (
            PathBuf::from(root).join("System32").join("cmd.exe"),
            vec!["/C".to_owned(), "set".to_owned()],
        )
    }
    #[cfg(not(windows))]
    {
        (PathBuf::from("/usr/bin/env"), Vec::new())
    }
}

/// A program that stays alive long enough to be observed and killed.
///
/// Every argument is one vector element, because the vector is passed as argv with no shell of ours to split
/// it — which is also the injection mitigation `docs/architecture/security.md` asks for, exercised here by the
/// tests rather than only claimed in a doc comment.
fn long_runner() -> (PathBuf, Vec<String>) {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        (
            PathBuf::from(root).join("System32").join("cmd.exe"),
            vec!["/C".to_owned(), "ping -n 60 127.0.0.1 > NUL".to_owned()],
        )
    }
    #[cfg(not(windows))]
    {
        (PathBuf::from("/bin/sleep"), vec!["60".to_owned()])
    }
}

/// The minimum allowlisted environment that lets [`long_runner`] actually run.
///
/// # This helper exists because a test found a real gap in the test, not in the crate
///
/// The first version of the kill test launched `cmd.exe` with an **empty** environment — correct for the
/// allowlist, and fatal for the runner: `cmd.exe` could not resolve `ping` without `PATH`, so the child exited
/// almost immediately and the "is this process alive" assertion failed. The failure was reported as the guard
/// not working.
///
/// The fix is not to weaken the allowlist but to state the one the runner needs, which is the more useful
/// assertion anyway: a request's environment must be able to be **sufficient**, not only restrictive. That is
/// exactly what an operator configuring an MCP server has to do, so the tests now exercise it.
fn long_runner_environment() -> BTreeMap<String, String> {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        BTreeMap::from([
            ("SystemRoot".to_owned(), root.clone()),
            ("windir".to_owned(), root.clone()),
            // Only the system directory, rather than this process's whole `PATH`: an allowlist that copied the
            // host's search path would be the extension this crate exists to prevent.
            ("PATH".to_owned(), format!("{root}\\System32")),
        ])
    }
    #[cfg(not(windows))]
    {
        BTreeMap::new()
    }
}

/// A request that starts [`long_runner`] under the policy the tests are checking.
fn long_runner_request() -> SandboxRequest {
    let (program, arguments) = long_runner();
    let mut request = unconfined_request(program, arguments);
    request.environment = long_runner_environment();
    request
}

/// **The support set is exactly what this platform can enforce — never more.**
///
/// The claim `docs/architecture/security.md` makes — "reports effective guarantees rather than claiming parity"
/// — is only meaningful if a host without a facility reports nothing. On Windows and macOS the only correct
/// answer today is an **empty** set, because the facilities that exist need `unsafe` FFI this workspace forbids.
///
/// Falsified by mutation: returning any non-empty set from `UnconfinedBackend::support` fails here. The Linux
/// half cannot be reached during development on Windows, which is why the list itself is asserted in
/// `backend.rs::tests` instead — see that test's note.
#[test]
fn the_support_set_never_claims_more_than_the_platform_can_enforce() {
    let support = backend_for_host().support();
    #[cfg(not(target_os = "linux"))]
    {
        assert!(
            support.is_empty(),
            "no non-Linux backend is implemented, so this host must claim nothing, got: {:?}",
            support.guarantees()
        );
        assert_eq!(
            backend_for_host().facility().as_str(),
            "unconfined",
            "the facility label must match the empty support set"
        );
    }
    #[cfg(target_os = "linux")]
    {
        // `CpuTimeCeiling` is the one guarantee cgroup v2 cannot express: it accounts CPU time in `cpu.stat` but
        // has no limit file for a cumulative total, only the rate form in `cpu.max`. Asserting its **absence**
        // is the load-bearing half, because claiming it is the specific mistake that would let a caller believe a
        // child was stopped after N seconds of CPU when it is only throttled.
        assert!(
            !support.supports(Guarantee::CpuTimeCeiling),
            "cgroup v2 has no cumulative CPU limit, so this must never be claimed"
        );
        if support.is_empty() {
            assert!(
                !std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
                "a v2 host must report guarantees; an empty set here means the probe failed to find a \
                 delegation, which is a finding about the probe rather than about the platform"
            );
        }
    }
}

/// **A guarantee this host cannot enforce is refused, naming it — the crate's fail-closed property.**
///
/// Written against `backend_for_host()` rather than a stub, so the test is about this machine. The guarantee to
/// refuse is **derived** from the host's support set rather than hard-coded, so the test cannot pass by accident
/// on a platform whose support set happens to be a superset of the guess.
///
/// Falsified by mutation: making `SandboxPolicy::new` ignore `required` fails here, and so does giving
/// `UnconfinedBackend` a non-empty support set.
#[test]
fn a_guarantee_this_host_cannot_enforce_is_refused_and_named() {
    let backend = backend_for_host();
    let support = backend.support();
    let all = [
        Guarantee::TreeTermination,
        Guarantee::ProcessCountCeiling,
        Guarantee::MemoryCeiling,
        Guarantee::CpuRateCeiling,
        Guarantee::CpuTimeCeiling,
    ];
    let Some(unenforceable) = all
        .into_iter()
        .find(|guarantee| !support.supports(*guarantee))
    else {
        // Every guarantee is enforceable, so this host has nothing to refuse. Reported rather than silently
        // passed: a quietly skipped assertion is the failure mode this suite is written against, and a bare
        // `return` here would look identical to a check that ran.
        eprintln!(
            "jarvis-sandbox: this host enforces every guarantee {:?}; the refusal path is untested here",
            support.guarantees()
        );
        return;
    };
    let (program, arguments) = long_runner();
    let mut request = unconfined_request(program, arguments);
    request.isolation = Isolation::Restricted;
    request.required = vec![unenforceable];
    let error = SandboxPolicy::new(request, backend.as_ref())
        .err()
        .unwrap_or_else(|| {
            panic!("{unenforceable} is not supported here, so this must be refused")
        });
    assert_eq!(error.unsupported(), Some(unenforceable));
    assert!(
        error.to_string().contains(unenforceable.as_str()),
        "the refusal must name the guarantee, got: {error}"
    );
}

/// **A launch starts a real, running process, and killing it really terminates that process.**
///
/// The unconfined path is asserted because it is the only launch path this host has — and because it is the
/// **fallback** every other backend shares: a launch that silently returned `Ok` for a process that never
/// started, or a pid of `0`, would make every confinement test built on top of it measure nothing.
///
/// # Why the child is checked through the operating system and not through the handle
///
/// The first version of this test asserted only that `kill()` returned `Ok`, which a **no-op** `kill` would also
/// satisfy — a guard with no real coverage. It now asks the kernel whether a process with that pid still exists,
/// so a `kill` that does nothing is caught.
///
/// Falsified by mutation: making `ChildHandle::kill` return `Ok(())` without killing fails here (the process is
/// still alive); returning a fixed pid from `launch` fails the same way.
#[tokio::test]
async fn a_launch_starts_a_real_process_that_the_kill_really_terminates() {
    let backend = backend_for_host();
    let (program, arguments) = long_runner();
    // Named `spawn` rather than `launcher`, because a binding that close to `launched` trips
    // `clippy::similar_names` — and the two really are easy to confuse in a test that reasons about both.
    let spawn = jarvis_sandbox::launcher_for(&long_runner_request(), true);
    let policy = SandboxPolicy::new(long_runner_request(), backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));
    assert!(
        program.exists(),
        "the test needs a real program at {program:?} for arguments {arguments:?}"
    );

    let launched = backend
        .launch(&policy, spawn)
        .await
        .unwrap_or_else(|error| {
            panic!("an unconfined launch of a real program must succeed: {error}")
        });
    let pid = launched.process.pid();
    assert_ne!(
        pid, 0,
        "a launch that reports pid 0 has not started anything"
    );
    assert!(launched.stdout.is_some(), "piped stdio was requested");
    assert!(
        launched.enforced.is_empty(),
        "nothing was required, so nothing is in force"
    );
    assert!(
        process_is_alive(pid),
        "the child must be running before it is killed, or this test proves nothing about the kill"
    );

    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing a process we own must succeed: {error}"));

    // The kernel is the authority, not the handle we just consumed.
    assert!(
        !process_is_alive(pid),
        "pid {pid} is still alive after a successful kill, so the kill did nothing"
    );
}

/// Returns whether a process with this id is currently alive.
///
/// A `kill(pid, 0)`-equivalent that needs no `libc`: `tasklist` on Windows and a presence check on `/proc` on
/// Linux and macOS. Chosen over an `unsafe` syscall because this workspace forbids `unsafe`, and over a
/// dependency because two commands are not worth one.
fn process_is_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let systemroot = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        let output = std::process::Command::new(
            PathBuf::from(systemroot)
                .join("System32")
                .join("tasklist.exe"),
        )
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output();
        match output {
            // `tasklist` prints "INFO: No tasks are running..." with no rows when nothing matches, and a row
            // containing the pid when something does. Matching the pid as a bare number could also match a
            // memory figure, so the check is for it surrounded by the column separator tasklist uses.
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout);
                text.contains(&format!(" {pid} ")) || text.contains(&format!("\t{pid}\t"))
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// **The child's environment is exactly the request's map — an allowlist, not an extension.**
///
/// `docs/architecture/security.md` asks for an "environment allowlist". The failure this catches is the one that
/// looks like success: an *added* sentinel is visible whether or not `env_clear` ran, so a sentinel-only
/// assertion passes while the child still inherits every host credential. The load-bearing half is therefore the
/// **absence** of an inherited variable — and the sentinel is kept as the positive control, so a launch that
/// produced no output at all cannot pass by producing nothing to match.
///
/// Falsified by mutation: removing `.env_clear()` makes the inherited variable appear and fails here; removing
/// `.envs()` makes the sentinel disappear and fails it too.
#[tokio::test]
async fn the_child_environment_is_the_requests_map_and_nothing_else() {
    let backend = backend_for_host();
    let (program, arguments) = env_dump();
    let mut request = unconfined_request(program, arguments);
    request
        .environment
        .insert("JARVIS_SANDBOX_SENTINEL".to_owned(), "present".to_owned());

    // A variable that exists in this process and that no program invents on its own, so its presence in the
    // child's output can only come from inheritance.
    #[cfg(windows)]
    let inherited = "USERPROFILE";
    #[cfg(not(windows))]
    let inherited = "HOME";

    let spawn = jarvis_sandbox::launcher_for(&request, true);
    let policy = SandboxPolicy::new(request, backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));
    let mut launched = backend
        .launch(&policy, spawn)
        .await
        .unwrap_or_else(|error| panic!("dumping the environment must succeed: {error}"));

    let mut stdout = launched
        .stdout
        .take()
        .unwrap_or_else(|| panic!("piped stdout was requested"));
    let mut dump = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut dump)
        .await
        .unwrap_or_else(|error| panic!("reading the child's output must succeed: {error}"));
    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing the child must succeed: {error}"));

    assert!(
        dump.contains("JARVIS_SANDBOX_SENTINEL"),
        "the request's own variable must reach the child; a launch that produced nothing cannot count as a \
         clean environment. got: {dump}"
    );
    assert!(
        !dump.contains(inherited),
        "the child inherited {inherited}, so the environment is an extension rather than an allowlist; got: {dump}"
    );
}

/// **The working directory in the request is applied to the child, and is neither dropped nor confused with the
/// daemon's own directory.**
///
/// A confinement claim is only meaningful for the child that was actually asked for. A backend that dropped the
/// working directory would run the operator's command somewhere else, and any relative path it wrote would land
/// in the daemon's directory instead.
///
/// The assertion is a **difference** as well as a value: the temp directory is confirmed to differ from this
/// process's directory first, so a backend that hard-coded the daemon's directory could not pass by coincidence.
///
/// Falsified by mutation: removing the `current_dir` call fails here.
#[tokio::test]
async fn the_requested_working_directory_is_applied() {
    let backend = backend_for_host();
    let working = std::env::temp_dir();
    let own = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    assert_ne!(
        working.canonicalize().ok(),
        own.canonicalize().ok(),
        "this test needs a directory different from the daemon's, or dropping `current_dir` would pass"
    );

    #[cfg(windows)]
    let (program, arguments) = {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_owned());
        (
            PathBuf::from(root).join("System32").join("cmd.exe"),
            vec!["/C".to_owned(), "cd".to_owned()],
        )
    };
    #[cfg(not(windows))]
    let (program, arguments) = (PathBuf::from("/bin/pwd"), Vec::new());

    let mut request = unconfined_request(program, arguments);
    request.working_directory = Some(working.clone());

    let spawn = jarvis_sandbox::launcher_for(&request, true);
    let policy = SandboxPolicy::new(request, backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));
    let mut launched = backend
        .launch(&policy, spawn)
        .await
        .unwrap_or_else(|error| panic!("reporting the working directory must succeed: {error}"));

    let mut stdout = launched
        .stdout
        .take()
        .unwrap_or_else(|| panic!("piped stdout was requested"));
    let mut reported = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stdout, &mut reported)
        .await
        .unwrap_or_else(|error| panic!("reading the child's output must succeed: {error}"));
    launched
        .process
        .kill()
        .await
        .unwrap_or_else(|error| panic!("killing the child must succeed: {error}"));

    // The final component is asserted rather than the whole path, because Windows reports the same directory in
    // a short or long form (`C:\Users\RUNNER~1\…`) depending on how it was passed, and a whole-path comparison
    // would fail a correct launch. `map_or_else` rather than `map(...).unwrap_or_else(...)`, because the
    // workspace forbids `unwrap_used` and clippy reads the two-step form as a missed simplification.
    let expected = working.file_name().map_or_else(
        || panic!("the temp dir must have a final component"),
        |name| name.to_string_lossy().into_owned(),
    );
    assert!(
        reported.contains(&expected),
        "the child must start in {working:?}, got: {reported}"
    );
}

/// **A permitted launch reaches the launcher, and a launcher failure is reported as a launch failure.**
///
/// The launcher is injected precisely so the ordering "confine, then spawn" can be asserted, and the
/// `refusing_launcher` records whether it was called at all. That record is exact where a process-table
/// inspection would be racy.
///
/// The error **kind** is asserted too, because the two kinds are not interchangeable at a call site: `Setup`
/// means the facility is missing and the operator must fix the host, while `Launch` means the facility worked
/// and the program could not be started — different actions, so collapsing them would mislead.
///
/// Falsified by mutation: a backend returning `Ok` without calling the launcher fails the first assertion;
/// mapping the launcher error to `SandboxError::Setup` fails the second.
#[tokio::test]
async fn the_launcher_is_called_exactly_when_a_launch_is_permitted() {
    let backend = backend_for_host();
    let observed = Arc::new(AtomicBool::new(false));
    let policy = SandboxPolicy::new(long_runner_request(), backend.as_ref())
        .unwrap_or_else(|error| panic!("a request requiring nothing must be accepted: {error}"));

    let outcome = backend
        .launch(&policy, refusing_launcher(Arc::clone(&observed)))
        .await;
    assert!(
        observed.load(Ordering::SeqCst),
        "a permitted launch must reach the launcher"
    );
    match outcome {
        Err(SandboxError::Launch { .. }) => {}
        Err(other) => {
            panic!("a launcher failure must be reported as a launch failure, got: {other}")
        }
        Ok(_) => panic!("the refusing launcher cannot produce a running child"),
    }
}
