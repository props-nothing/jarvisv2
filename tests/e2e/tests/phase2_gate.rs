//! Phase 2 acceptance gate: the roadmap exit gate, executed as a test.
//!
//! `ROADMAP.md` Phase 2 exit gate: "a run streams to CLI, survives daemon restart at a persisted
//! boundary, can be cancelled, and is fully testable without network credentials."
//!
//! This is a **process-level** gate, like `phase1_gate`. It runs the real `jarvisd` and the real
//! `jarvis` client against a temporary portable root, because the library suites prove crates and
//! this gate proves an installation. Every credential it uses is the one the daemon issues itself,
//! and no provider is contacted, so the whole gate runs offline.
//!
//! Each step is asserted separately so a failure names the step that broke rather than reporting
//! "the gate failed". The gate skips with a clear message when the binaries are absent, so
//! `cargo test` stays useful before a build exists; CI sets `ACCEPTANCE_REQUIRE_BINARIES=1` to make
//! that skip a failure. The variable deliberately avoids the `JARVIS_` prefix, because the daemon
//! rejects unknown `JARVIS_*` variables by design and a harness variable in that namespace would
//! stop the daemon from starting at all.
//!
//! # Why the whole gate is `async`
//!
//! HTTP calls must be made from a task running **inside** a runtime, not merely awaited by one.
//! The first version of this gate drove a synchronous body through `Runtime::block_on` and every
//! `reqwest` request panicked with "there is no reactor running": `reqwest` asks for the current
//! handle to build its deadline, and neither `block_on` nor entering the runtime around it gives the
//! future that context when the *request builder* is evaluated outside it. Making the gate async
//! removes the distinction entirely, so there is no bridge left to get wrong. The single blocking
//! concern — process control — is synchronous in the standard library and needs no bridge.
//!
//! # How the restart is produced
//!
//! A run must be **in flight** when the process dies, because that is what recovery exists for. The
//! gate produces one through the public API rather than by writing rows: the first daemon is started
//! with **no executor configured**, which is the supported configuration `P2-007` shipped and in
//! which runs are accepted and recorded but never advanced. The runs are therefore created by the
//! real gateway, through the real request path, and are left non-terminal; the daemon is then killed
//! abruptly, so nothing remains that could advance them.
//!
//! That reproduces the **durable** state a mid-run crash leaves — a non-terminal run row with its
//! events and no process behind it — which is the only state recovery can observe. The in-memory
//! part of an interrupted run (a parked model call, a partial answer) is deliberately not simulated,
//! because the daemon cannot recover it either, and the honest behaviour is to say so.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use jarvis_storage::{AppPaths, portable_layout};

/// The published run API path prefix.
///
/// Written out rather than imported from the crate that defines it. A gate that asked the product
/// where its own endpoints are could not notice a path change, and the endpoint is part of the
/// contract this gate exists to hold still.
const API_PREFIX: &str = "/api/v1";
/// Seconds to wait for a daemon to report readiness before failing the gate.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// Seconds to wait for a process to exit after a termination request.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
/// Seconds to wait for a run stream to close.
const STREAM_DEADLINE: Duration = Duration::from_secs(30);
/// Poll interval while waiting for a condition.
const POLL: Duration = Duration::from_millis(50);
/// Environment variable that turns a missing-binary skip into a failure.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";
/// The loopback port the gate tells the daemon to serve on.
///
/// Fixed rather than ephemeral because the daemon's configuration validation refuses port `0`: an
/// ephemeral port would leave a client with no address to name. It is high and unusual so a
/// developer's own daemon is unlikely to hold it, and an occupied port fails the gate loudly at the
/// bind rather than silently testing something else.
const GATE_HTTP_PORT: u16 = 43_871;
/// The states a run must never be left in once a daemon is serving.
const NON_TERMINAL: [&str; 7] = [
    "received",
    "context_building",
    "planning",
    "executing",
    "observing",
    "awaiting_approval",
    "responding",
];
/// The terminal event names a settled run's stream ends with.
const TERMINAL_EVENTS: [&str; 3] = ["run_completed", "run_cancelled", "run_failed"];

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Removes the gate's temporary root when the test ends, pass or fail.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jarvis-phase2-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        Self(path)
    }

    fn paths(&self) -> AppPaths {
        portable_layout(&self.0).unwrap_or_else(|| panic!("portable root must be absolute"))
    }

    /// Returns the daemon's structured log.
    fn log(&self) -> String {
        std::fs::read_to_string(self.paths().logs().join("jarvisd.jsonl")).unwrap_or_default()
    }

    /// Writes the profile configuration the gate needs.
    ///
    /// The directory is prepared first, because `config.toml`'s parent does not exist in a fresh
    /// portable root and the daemon is the process that would otherwise create it. A gate that
    /// depended on the daemon to create the file the daemon reads could not start a daemon at all.
    ///
    /// `executor` selects whether runs are driven. With it absent, runs are accepted and recorded
    /// but never advanced — the configuration `P2-007` shipped, and the one that lets the gate
    /// create genuinely interrupted runs.
    fn write_config(&self, executor: bool) {
        let paths = self.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("prepare the profile directories: {error}"));
        let executor_line = if executor {
            "executor_model = \"scripted\"\n"
        } else {
            ""
        };
        let document = format!(
            "schema_version = 1\n\
             \n\
             [profile]\n\
             name = \"gate\"\n\
             \n\
             [logging]\n\
             level = \"info\"\n\
             \n\
             [daemon]\n\
             shutdown_timeout_seconds = 15\n\
             http_enabled = true\n\
             http_port = {GATE_HTTP_PORT}\n\
             {executor_line}"
        );
        std::fs::write(paths.config().join("config.toml"), document)
            .unwrap_or_else(|error| panic!("write config: {error}"));
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Locates a built binary beside the current test executable.
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

    let message = "the phase 2 gate requires built jarvisd and jarvis binaries";
    assert!(
        std::env::var_os(REQUIRE_BINARIES_ENV).is_none(),
        "{message}; run `cargo build --workspace` first"
    );
    eprintln!("SKIP: {message}; run `cargo build --workspace` first");
    None
}

/// A running daemon that is terminated when dropped.
struct RunningDaemon {
    child: Child,
}

impl RunningDaemon {
    fn start(daemon: &Path, root: &Path) -> Self {
        let child = Command::new(daemon)
            .arg("--root")
            .arg(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("start daemon: {error}"));
        Self { child }
    }

    fn process_id(&self) -> u32 {
        self.child.id()
    }

    /// Waits until *this* daemon records a `ready` lifecycle row.
    ///
    /// Scoped to the process identifier that was just started, because a hard-terminated daemon
    /// never runs its `stopped` transition: its last recorded state stays `ready`, so a looser check
    /// would accept a stale row before the new process had done anything.
    async fn wait_until_ready(&mut self, root: &Path) {
        let database = portable_layout(root)
            .unwrap_or_else(|| panic!("portable root"))
            .data()
            .join("jarvis.sqlite3");
        let deadline = Instant::now() + READY_DEADLINE;
        let expected = i64::from(self.process_id());

        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|error| panic!("poll daemon: {error}"))
            {
                panic!("daemon exited early with {status}: {}", self.drain_stderr());
            }
            if let Ok(inspection) = jarvis_storage::inspect_database(&database).await
                && inspection
                    .daemon_instances
                    .iter()
                    .any(|row| row.state == "ready" && row.process_id == expected)
            {
                return;
            }
            tokio::time::sleep(POLL).await;
        }
        panic!("the daemon did not become ready within {READY_DEADLINE:?}");
    }

    /// Kills the process abruptly and returns whatever it wrote to stderr.
    ///
    /// Abrupt rather than graceful, which is the point: a graceful shutdown drains in-flight work, so
    /// it would leave no interrupted run to recover. The wait is blocking because process control is
    /// synchronous in the standard library; it cannot starve the runtime because the gate is a single
    /// task.
    fn kill_and_drain(&mut self) -> String {
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
        panic!("the daemon did not exit within {STOP_DEADLINE:?}");
    }

    fn drain_stderr(&mut self) -> String {
        let mut stderr = String::new();
        if let Some(mut stream) = self.child.stderr.take() {
            let _ = stream.read_to_string(&mut stderr);
        }
        stderr
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads the profile credential the daemon issued.
///
/// The file holds the token alone, so it is trimmed rather than parsed. Reading it here rather than
/// through the storage crate is deliberate: the gate must consume the credential exactly as an
/// unrelated client would, and a shared parser would agree with the writer by construction.
fn profile_credential(root: &TempRoot) -> String {
    let path = root
        .paths()
        .config()
        .join(jarvis_storage::CREDENTIAL_FILE_NAME);
    let token = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read the profile credential: {error}"));
    let token = token.trim();
    assert!(
        !token.is_empty() && !token.contains(char::is_whitespace),
        "the credential file must hold one token"
    );
    token.to_owned()
}

/// A minimal authenticated client for the run API.
struct RunClient {
    base: String,
    credential: String,
    http: reqwest::Client,
}

impl RunClient {
    fn new(credential: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            // No client-level timeout for the same reason the CLI has none: it bounds the whole
            // response body, so it would terminate a stream at the deadline.
            .read_timeout(STREAM_DEADLINE)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|error| panic!("build http client: {error}"));
        Self {
            base: format!("http://127.0.0.1:{GATE_HTTP_PORT}{API_PREFIX}"),
            credential,
            http,
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
    }

    /// Starts a run and returns its identifier and version.
    async fn start_run(&self, objective: &str) -> (String, i64) {
        let response = self
            .request(reqwest::Method::POST, "/runs")
            .json(&serde_json::json!({ "objective": objective }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("start run: {error}"));
        assert_eq!(
            response.status().as_u16(),
            201,
            "a run start must be accepted"
        );
        let body: serde_json::Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("decode run reply: {error}"));
        (
            string_field(&body, "run_id"),
            body["version"]
                .as_i64()
                .unwrap_or_else(|| panic!("a run reply must carry version: {body}")),
        )
    }

    /// Reads one run.
    async fn read_run(&self, run_id: &str) -> serde_json::Value {
        let path = format!("/runs/{run_id}");
        let response = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await
            .unwrap_or_else(|error| panic!("read run: {error}"));
        assert_eq!(
            response.status().as_u16(),
            200,
            "a known run must be readable"
        );
        response
            .json()
            .await
            .unwrap_or_else(|error| panic!("decode run: {error}"))
    }

    /// Requests cancellation and returns the status the daemon answered with.
    ///
    /// Sends **no body**: a cancellation is operator intent and carries no version, because the run's
    /// own progress writes invalidate a client's version almost immediately. See
    /// `jarvis_storage::request_run_cancellation`.
    async fn cancel_run(&self, run_id: &str) -> u16 {
        let path = format!("/runs/{run_id}/cancel");
        let response = self
            .request(reqwest::Method::POST, &path)
            .send()
            .await
            .unwrap_or_else(|error| panic!("cancel run: {error}"));
        response.status().as_u16()
    }

    /// Opens a run's stream and returns its `event:` names in arrival order.
    ///
    /// The sequence is the contract: a names-only comparison would pass for a client that lost an
    /// event, so the callers below assert positions and counts rather than membership.
    async fn read_stream(&self, run_id: &str) -> Vec<String> {
        let path = format!("/runs/{run_id}/stream");
        let mut response = self
            .request(reqwest::Method::GET, &path)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .unwrap_or_else(|error| panic!("open stream: {error}"));
        assert_eq!(
            response.status().as_u16(),
            200,
            "an existing run's stream must open"
        );

        // Read with a deadline, so a stream that never closes fails the gate rather than hanging it.
        let deadline = tokio::time::Instant::now() + STREAM_DEADLINE;
        let mut buffer = String::new();
        loop {
            // The outer error is the deadline elapsing, so it is named rather than discarded: an
            // unnamed `Err(_)` would also swallow a genuinely different failure.
            let elapsed = tokio::time::timeout_at(deadline, response.chunk()).await;
            match elapsed {
                Err(tokio::time::error::Elapsed { .. }) => {
                    panic!("the run stream did not close within {STREAM_DEADLINE:?}");
                }
                Ok(Ok(None)) => break,
                Ok(Ok(Some(bytes))) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
                Ok(Err(error)) => panic!("read stream: {error}"),
            }
        }

        buffer
            .lines()
            .filter_map(|line| line.strip_prefix("event: "))
            .map(str::to_owned)
            .collect()
    }
}

/// Reads a required string field from a decoded reply.
fn string_field(body: &serde_json::Value, name: &str) -> String {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("the reply must carry {name}: {body}"))
        .to_owned()
}

/// Asserts a run has not settled.
fn assert_in_flight(body: &serde_json::Value, context: &str) {
    let state = string_field(body, "state");
    assert!(
        NON_TERMINAL.contains(&state.as_str()),
        "{context} must still be in flight, found {state}: {body}"
    );
}

/// Runs the gate against explicit binaries and returns the daemon's stderr.
///
/// # Panics
///
/// Panics with the failing step named when any acceptance step fails.
#[must_use]
pub async fn run_gate(daemon: &Path, client: &Path) -> String {
    let root = TempRoot::new("gate");
    // No executor: runs are accepted and recorded but never advanced, which is how the gate obtains
    // runs that are genuinely in flight.
    root.write_config(false);

    let mut first = RunningDaemon::start(daemon, &root.0);
    first.wait_until_ready(&root.0).await;
    let run_client = RunClient::new(profile_credential(&root));

    let cancellation = assert_cancellation_is_a_request(&run_client).await;
    // A second run is left alone entirely, so the restart has one run with nothing behind it at all.
    let (abandoned, _) = run_client
        .start_run("this run will be interrupted by a restart")
        .await;
    assert_in_flight(
        &run_client.read_run(&abandoned).await,
        "the abandoned run before the restart",
    );

    let stderr = first.kill_and_drain();

    // The restart both accounts for the interrupted runs and serves new ones, so `executor_model` is
    // set for this start. Setting it between starts is itself meaningful: the executor is resolved
    // from configuration at every startup rather than fixed for the process's lifetime.
    root.write_config(true);
    let mut second = RunningDaemon::start(daemon, &root.0);
    second.wait_until_ready(&root.0).await;

    assert_restart_settled_the_abandoned_run(&run_client, &abandoned).await;
    assert_restart_honoured_the_cancellation(&run_client, &cancellation).await;
    assert_the_recovery_is_logged(&root, &abandoned);
    assert_a_run_streams_to_a_terminal_state(&run_client).await;
    assert_the_cli_can_ask(client, &root);

    let restarted_stderr = second.kill_and_drain();
    format!("{stderr}\n{restarted_stderr}")
}

#[tokio::test]
async fn phase2_gate_proves_runs_survive_restart_on_this_platform() {
    let Some((daemon, client)) = required_binaries() else {
        return;
    };
    let _stderr = run_gate(&daemon, &client).await;
}

/// Proves a cancellation is recorded as a request while the run stays in flight.
async fn assert_cancellation_is_a_request(client: &RunClient) -> String {
    let (run_id, _version) = client.start_run("cancel this run").await;
    assert_eq!(
        client.cancel_run(&run_id).await,
        200,
        "a cancellation needs no version, so it is accepted while the run is in flight"
    );
    let observed = client.read_run(&run_id).await;
    assert_in_flight(&observed, "a cancelled-before-restart run");
    assert!(
        observed["cancellation_requested_at"].is_string(),
        "the request must be recorded on the run: {observed}"
    );
    run_id
}

/// Proves the restarted daemon settled the run that had nothing behind it.
async fn assert_restart_settled_the_abandoned_run(client: &RunClient, run_id: &str) {
    let settled = client.read_run(run_id).await;
    assert_eq!(
        string_field(&settled, "state"),
        "failed",
        "a run interrupted by a restart must be reported as failed: {settled}"
    );
    assert_eq!(
        string_field(&settled, "error_code"),
        jarvis_storage::INTERRUPTED_ERROR_CODE,
        "the failure must name the restart as its cause: {settled}"
    );
    assert!(
        settled["completed_at"].is_string(),
        "a settled run must carry its completion time: {settled}"
    );

    // The settlement is in the stream a client replays, not only in the row. A client that reconnects
    // after the restart learns the outcome from the same place it learned everything else.
    let events = client.read_stream(run_id).await;
    assert_eq!(
        events.last().map(String::as_str),
        Some("run_failed"),
        "the replayed stream must end with the settlement: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|name| TERMINAL_EVENTS.contains(&name.as_str()))
            .count(),
        1,
        "a restart must settle a run exactly once: {events:?}"
    );
}

/// Proves the operator's cancellation outranks the interruption.
///
/// Settling this run `failed` would report a daemon fault for work the operator had already stopped.
async fn assert_restart_honoured_the_cancellation(client: &RunClient, run_id: &str) {
    let settled = client.read_run(run_id).await;
    assert_eq!(
        string_field(&settled, "state"),
        "cancelled",
        "a run whose cancellation was requested must settle as cancelled: {settled}"
    );
    assert!(
        settled["error_code"].is_null(),
        "a cancellation is not a failure and must carry no error code: {settled}"
    );

    let events = client.read_stream(run_id).await;
    assert_eq!(
        events.last().map(String::as_str),
        Some("run_cancelled"),
        "the replayed stream must end with the cancellation: {events:?}"
    );
}

/// Proves recovery is visible to an operator rather than silent.
///
/// A run that changed state without anything saying why is indistinguishable from corruption, and
/// the daemon's log is where the reason is recorded.
fn assert_the_recovery_is_logged(root: &TempRoot, run_id: &str) {
    let log = root.log();
    assert!(
        log.contains(run_id),
        "the daemon must log the runs it recovered; {run_id} is absent from the log"
    );
    assert!(
        log.contains("settled runs interrupted by a previous process"),
        "the daemon must log why those runs were settled"
    );
}

/// Proves a run reaches a terminal state and its stream reports the whole sequence.
async fn assert_a_run_streams_to_a_terminal_state(client: &RunClient) {
    let (run_id, _) = client.start_run("what is on the agenda today").await;
    let events = client.read_stream(&run_id).await;

    assert!(
        events.iter().any(|name| name == "state_changed"),
        "the stream must report state changes: {events:?}"
    );
    assert_eq!(
        events.last().map(String::as_str),
        Some("run_completed"),
        "the stream must end with the terminal event: {events:?}"
    );

    let settled = client.read_run(&run_id).await;
    assert_eq!(
        string_field(&settled, "state"),
        "completed",
        "the run must settle as completed: {settled}"
    );
    assert_eq!(
        string_field(&settled, "objective"),
        "what is on the agenda today",
        "the run must carry the objective it was started with"
    );
}

/// Proves the CLI's `ask` reaches the same API over the same profile.
///
/// The CLI is the interface the exit gate names, so a run that only this gate's own client can read
/// would not satisfy it.
fn assert_the_cli_can_ask(client: &Path, root: &TempRoot) {
    let output = Command::new(client)
        .args(["ask", "summarise the last message", "--root"])
        .arg(&root.0)
        .output()
        .unwrap_or_else(|error| panic!("run the client: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "ask must succeed: {stdout} {stderr}"
    );
    assert!(
        stdout.contains("run "),
        "ask must report the accepted run: {stdout}"
    );
}
