//! Phase 1 acceptance gate: the roadmap exit gate, executed as a test.
//!
//! `ROADMAP.md` Phase 1 exit gate: "on all three operating systems, a clean build
//! starts the daemon, the CLI reaches health over local transport, state survives
//! restart, and `doctor` diagnoses a deliberately broken configuration."
//!
//! This is a **process-level** gate. Every assertion runs the real `jarvisd` and
//! `jarvis` binaries against a temporary portable root, because the unit and
//! contract suites prove libraries, not an installation
//! (`docs/development/testing.md`: "Platform" tests must run on native
//! environments rather than be emulated from one OS).
//!
//! Each step is asserted separately so a failure names the step that broke rather
//! than reporting "the gate failed". The gate is skipped with a clear message when
//! the binaries are absent, so `cargo test` stays useful before a build exists; CI
//! sets `ACCEPTANCE_REQUIRE_BINARIES=1` to turn that skip into a failure. The name
//! avoids the `JARVIS_` prefix on purpose: the daemon rejects unknown `JARVIS_*`
//! variables, so a harness variable there would stop the daemon from starting.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use jarvis_storage::{AppPaths, DatabaseState, inspect_database, portable_layout};

/// Seconds to wait for a daemon to report readiness before failing the gate.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// Seconds to wait for a process to exit after a termination request.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
/// Poll interval while waiting for a condition.
const POLL: Duration = Duration::from_millis(100);
/// Environment variable that turns a missing-binary skip into a failure.
///
/// It must not begin with `JARVIS_`, because the daemon treats every unknown
/// `JARVIS_*` variable as a configuration error and would refuse to start.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";

/// Removes the gate's temporary root when the test ends, pass or fail.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jarvis-acceptance-{label}-{}",
            jarvis_core::scratch_tag()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        Self(path)
    }

    fn paths(&self) -> AppPaths {
        portable_layout(&self.0).unwrap_or_else(|| panic!("portable root must be absolute"))
    }

    fn database(&self) -> PathBuf {
        self.0.join("data").join("jarvis.sqlite3")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Locates a built binary beside the current test executable.
///
/// Cargo places integration-test binaries in `target/<profile>/deps`, so the
/// application binaries sit one directory up in `target/<profile>`.
fn binary(name: &str) -> Option<PathBuf> {
    let mut directory = std::env::current_exe().ok()?;
    directory.pop();
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory.pop();
    }
    let candidate = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

fn required_binaries() -> Option<(PathBuf, PathBuf)> {
    let daemon = binary("jarvisd");
    let client = binary("jarvis");
    if let (Some(daemon), Some(client)) = (daemon, client) {
        return Some((daemon, client));
    }

    let message = "acceptance gate requires built jarvisd and jarvis binaries";
    // The variable deliberately does NOT use the `JARVIS_` prefix: the daemon's
    // configuration loader rejects any unknown `JARVIS_*` variable by design, so a
    // harness variable in that namespace stops the daemon from starting at all.
    assert!(
        std::env::var_os(REQUIRE_BINARIES_ENV).is_none(),
        "{message}; run `cargo build --workspace` first"
    );
    eprintln!("SKIP: {message}; run `cargo build --workspace` first");
    None
}

fn run_client(client: &Path, root: &Path, arguments: &[&str]) -> (bool, String, String) {
    let output = Command::new(client)
        .args(arguments)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap_or_else(|error| panic!("run {}: {error}", client.display()));
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A running daemon that is terminated when dropped.
struct RunningDaemon {
    child: Child,
}

impl RunningDaemon {
    fn start(daemon: &Path, root: &Path, extra: &[&str]) -> Self {
        let child = Command::new(daemon)
            .arg("--root")
            .arg(root)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start daemon: {error}"));
        Self { child }
    }

    /// Waits until *this* daemon records a `ready` lifecycle row.
    ///
    /// Readiness is proven from durable state, not stdout, and it is scoped to the
    /// process ID that was just started. Matching any `ready` row would accept a
    /// stale row: a hard-terminated daemon never runs its `stopped` transition, so
    /// its last recorded state stays `ready` and would satisfy a looser check
    /// before the new process had done anything.
    fn wait_until_ready(
        &mut self,
        root: &Path,
        process_id: u32,
    ) -> Vec<jarvis_storage::DaemonInstanceRow> {
        let paths = portable_layout(root).unwrap_or_else(|| panic!("portable root"));
        let database = paths.data().join("jarvis.sqlite3");
        let deadline = Instant::now() + READY_DEADLINE;
        let expected_pid = i64::from(process_id);

        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("poll daemon: {error}"))
            {
                let mut stderr = String::new();
                if let Some(mut stream) = self.child.stderr.take() {
                    let _ = stream.read_to_string(&mut stderr);
                }
                panic!("daemon exited early with {status}: {}", stderr.trim());
            }

            // Inspection is read-only, so polling cannot create the database and
            // mask a daemon that never started.
            if let Ok(inspection) = futures_lite_block_on(inspect_database(&database))
                && inspection
                    .daemon_instances
                    .iter()
                    .any(|row| row.state == "ready" && row.process_id == expected_pid)
            {
                return inspection.daemon_instances;
            }
            std::thread::sleep(POLL);
        }
        panic!("daemon {process_id} did not become ready within {READY_DEADLINE:?}");
    }

    /// Requests termination, drains both streams, and waits for the process to exit.
    ///
    /// Both piped streams are drained **after** the process is terminated. Draining
    /// them while the daemon runs would block forever on EOF that only arrives when
    /// the process ends, which hangs the gate instead of failing it.
    fn stop(&mut self) -> String {
        let _ = self.child.kill();

        let mut stderr = String::new();
        if let Some(mut stream) = self.child.stderr.take() {
            let _ = stream.read_to_string(&mut stderr);
        }
        if let Some(mut stream) = self.child.stdout.take() {
            let mut discard = String::new();
            let _ = stream.read_to_string(&mut discard);
        }

        let deadline = Instant::now() + STOP_DEADLINE;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return stderr,
                Ok(None) => std::thread::sleep(POLL),
                Err(error) => panic!("poll daemon exit: {error}"),
            }
        }
        panic!("daemon did not exit within {STOP_DEADLINE:?}");
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs a future to completion on a private runtime.
///
/// The gate is synchronous so process control stays simple, so it needs a small
/// blocking bridge for the one async call it makes.
fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("build runtime: {error}"))
        .block_on(future)
}

/// Runs the gate against explicit binaries and returns the daemon's stderr.
///
/// This is the entry point CI uses, so the gate is not limited to binaries that
/// happen to sit beside the test executable. It asserts the same steps as
/// [`phase1_gate_proves_the_local_foundation_on_this_platform`].
///
/// # Panics
///
/// Panics with the failing step named when any acceptance step fails.
#[must_use]
pub fn run_gate(daemon: &Path, client: &Path) -> String {
    run_gate_steps(daemon, client)
}

fn run_gate_steps(daemon: &Path, client: &Path) -> String {
    let root = TempRoot::new("gate");
    let paths = root.paths();
    assert!(
        !root.database().exists(),
        "the gate must start from a clean profile"
    );
    assert_eq!(paths.mode(), jarvis_storage::PathMode::Portable);

    // Start: the daemon becomes ready and persists that state durably.
    let mut running = RunningDaemon::start(daemon, &root.0, &[]);
    let first_pid = running.child.id();
    let first_lifecycle = running.wait_until_ready(&root.0, first_pid);
    assert_eq!(
        first_lifecycle
            .iter()
            .filter(|row| row.state == "ready")
            .count(),
        1,
        "exactly one lifecycle should be ready after first start"
    );
    let initial_lifecycle_id = lifecycle_id(&first_lifecycle);

    // Transport: the CLI reaches the daemon and describes the same one.
    assert_health_and_status(client, &root.0, &initial_lifecycle_id);

    // Restart: stop first so the next start is a genuine restart, not a second
    // instance contending for the profile lock.
    running.stop();
    let mut restarted = RunningDaemon::start(daemon, &root.0, &[]);
    let second_pid = restarted.child.id();
    let second_lifecycle = restarted.wait_until_ready(&root.0, second_pid);
    assert_state_survives_restart(
        &first_lifecycle,
        &second_lifecycle,
        &initial_lifecycle_id,
        second_pid,
    );

    // Integrity: the schema is intact after two lifecycles.
    assert_schema_intact(&root);

    // Identity: the profile has the local user and workspace it needs to run
    // anything. Asserted here, against the database a real daemon produced, because
    // the storage tests supply those rows themselves and so cannot detect their
    // absence in a real profile.
    restarted.stop();
    assert_local_identity_seeded(&root);

    // Healthy diagnosis, then a deliberately broken configuration.
    let mut third = RunningDaemon::start(daemon, &root.0, &[]);
    let third_pid = third.child.id();
    third.wait_until_ready(&root.0, third_pid);
    assert_doctor_healthy(client, &root.0);
    let stderr = third.stop();
    assert_doctor_diagnoses_broken_configuration(client, &root.0, &paths);
    stderr
}

#[test]
fn phase1_gate_proves_the_local_foundation_on_this_platform() {
    let Some((daemon, client)) = required_binaries() else {
        return;
    };

    // A clean profile: nothing exists before the daemon starts.
    let _stderr = run_gate(&daemon, &client);
}

/// Returns the lifecycle identity, asserting one was recorded.
fn lifecycle_id(lifecycle: &[jarvis_storage::DaemonInstanceRow]) -> String {
    let row = lifecycle
        .first()
        .unwrap_or_else(|| panic!("the daemon must record a lifecycle row"));
    assert!(!row.id.is_empty(), "a lifecycle identity must not be empty");
    row.id.clone()
}

/// Proves the CLI reaches health over the local transport and sees the same daemon.
fn assert_health_and_status(client: &Path, root: &Path, expected_id: &str) {
    let (ok, stdout, stderr) = run_client(client, root, &["health", "--json"]);
    assert!(ok, "health failed: {} {}", stdout.trim(), stderr.trim());
    assert!(
        stdout.contains("\"ready\":true"),
        "health should report readiness: {}",
        stdout.trim()
    );

    let (ok, stdout, stderr) = run_client(client, root, &["status"]);
    assert!(ok, "status failed: {} {}", stdout.trim(), stderr.trim());
    assert!(
        stdout.contains(expected_id),
        "status must describe the running daemon: {}",
        stdout.trim()
    );
}

/// Proves the previous lifecycle row survives a restart instead of being replaced.
fn assert_state_survives_restart(
    first: &[jarvis_storage::DaemonInstanceRow],
    second: &[jarvis_storage::DaemonInstanceRow],
    first_id: &str,
    second_pid: u32,
) {
    assert!(
        second.len() > first.len(),
        "restart must add a lifecycle row, not replace the profile: {second:?}"
    );
    // The original lifecycle must still be present: state survives, it is not reset.
    assert!(
        second.iter().any(|row| row.id == first_id),
        "the previous lifecycle row must survive the restart"
    );
    // And the new row must belong to the process that just started.
    assert!(
        second
            .iter()
            .any(|row| row.process_id == i64::from(second_pid) && row.state == "ready"),
        "the restarted process must record its own lifecycle row: {second:?}"
    );
}

/// Proves the on-disk schema still matches this build and is not corrupt.
fn assert_schema_intact(root: &TempRoot) {
    let inspection = futures_lite_block_on(inspect_database(&root.database()))
        .unwrap_or_else(|error| panic!("inspect database: {error}"));
    assert_eq!(
        inspection.state,
        DatabaseState::Current {
            version: jarvis_storage::CURRENT_SCHEMA_VERSION
        },
        "schema should still match this build after restart"
    );
    assert!(inspection.integrity_ok, "database integrity must hold");
}

/// Proves a profile that ran a real daemon has the local identity it needs.
///
/// This is the process-level counterpart of the storage test for the same thing, and
/// it is the assertion that was missing when migration `0003` claimed to seed the
/// local user and workspace while no migration did. Every storage test passed, because
/// the fixtures wrote those rows themselves; a real profile had none, so no client
/// could create a run. Asserting it against the database a real `jarvisd` produced is
/// what makes that class of gap visible.
fn assert_local_identity_seeded(root: &TempRoot) {
    let database = futures_lite_block_on(jarvis_storage::SqliteDatabase::open(&root.database()))
        .unwrap_or_else(|error| panic!("open database: {error}"));
    let identity = futures_lite_block_on(jarvis_storage::load_local_identity(&database))
        .unwrap_or_else(|error| {
            panic!("a profile that ran a daemon must have a local identity: {error}")
        });

    assert_eq!(
        identity.user_id(),
        jarvis_storage::LOCAL_USER_ID,
        "the seeded local user must be the identity this build expects"
    );
    assert_eq!(
        identity.workspace_id(),
        jarvis_storage::LOCAL_WORKSPACE_ID,
        "the seeded local workspace must be the identity this build expects"
    );
    futures_lite_block_on(database.close());
}

/// Proves doctor reports a healthy installation with no blocking findings.
fn assert_doctor_healthy(client: &Path, root: &Path) {
    let (ok, stdout, stderr) = run_client(client, root, &["doctor"]);
    assert!(ok, "doctor failed: {} {}", stdout.trim(), stderr.trim());
    assert!(
        stdout.contains("mode=portable"),
        "doctor should report the portable mode: {}",
        stdout.trim()
    );
    assert!(
        stdout.contains("error=0"),
        "a healthy installation must have no error findings: {}",
        stdout.trim()
    );
}

/// Proves doctor diagnoses a broken configuration, names it, and does not repair it.
fn assert_doctor_diagnoses_broken_configuration(client: &Path, root: &Path, paths: &AppPaths) {
    let config = paths.config().join("config.toml");
    let broken = "schema_version = 1\nthis_key_does_not_exist = true\n";
    std::fs::write(&config, broken).unwrap_or_else(|error| panic!("write broken config: {error}"));

    let (ok, stdout, stderr) = run_client(client, root, &["doctor"]);
    assert!(
        !ok,
        "doctor must fail on a broken configuration, said: {} {}",
        stdout.trim(),
        stderr.trim()
    );
    assert!(
        stdout.contains("config.invalid"),
        "doctor must name the configuration finding: {}",
        stdout.trim()
    );
    assert!(
        stdout.contains("remediation:"),
        "every finding must carry a remediation: {}",
        stdout.trim()
    );

    // Diagnosis must not repair: the broken configuration is left exactly as found.
    let after =
        std::fs::read_to_string(&config).unwrap_or_else(|error| panic!("read config: {error}"));
    assert_eq!(
        after, broken,
        "doctor must not rewrite a broken configuration"
    );
}
