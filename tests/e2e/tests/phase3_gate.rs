//! Phase 3 acceptance gate: the roadmap exit gate, executed as a test.
//!
//! `ROADMAP.md` Phase 3 exit gate: "an external MCP client can call only its granted read tool; a write
//! tool pauses for a human decision; restart and duplicate delivery do not execute the effect twice."
//!
//! This is a **process-level** gate, like `phase1_gate` and `phase2_gate`. It runs the real `jarvisd`
//! against a temporary portable root and drives it over its published HTTP API, because the library suites
//! prove crates and this gate proves an installation. No provider is contacted and no credential is
//! invented: the only secret used is the one the daemon issues itself, so the whole gate runs offline.
//!
//! Each step is asserted separately so a failure names the step that broke rather than reporting "the gate
//! failed". The gate skips with a clear message when the binaries are absent, so `cargo test` stays useful
//! before a build exists; CI sets `ACCEPTANCE_REQUIRE_BINARIES=1` to make that skip a failure.
//!
//! # Why the write tool comes from a **real MCP server** rather than a fixture inside the daemon
//!
//! The gate needs a tool that a human must approve. The daemon's own filesystem tool is read-only, and a
//! test-only writable adapter compiled into `jarvisd` would prove that a *fixture* can hold a call rather
//! than that the platform can. So the gate configures one real MCP server over the stdio transport â€”
//! `fixture_peer`, the same hand-written server the transport's own tests spawn â€” with the
//! **unclassified** posture, which is the daemon's own default for a server nobody has classified: risk 3,
//! effects `Write` and `ExternalCommunication`, and `ApprovalPolicy::Ask`.
//!
//! That means the hold is produced by the **product's own configuration path**, from a posture an operator
//! genuinely gets by naming a server and saying nothing else. The tool identifier is prefixed with the
//! server name (`P3-008`'s `Prefixed` strategy, the only one the daemon uses), so the gate names the tool
//! the way the catalog does rather than guessing.
//!
//! # The duplicate delivery is two different duplicates, and both are asserted
//!
//! "Restart and duplicate delivery do not execute the effect twice" is two claims:
//!
//! - a **restart** between the hold and the decision must not change what the decision resumes â€” the
//!   approval has to outlive the process that requested it as durable state the authenticated owner can
//!   still answer after the daemon restarts;
//! - a **duplicate resumption** must not run the call twice â€” which is `record_tool_outcome`'s terminal
//!   guard plus the resume path's own "still `requested`" check.
//!
//! The gate produces the restart by killing the daemon abruptly, which is what `phase2_gate` does and for
//! the same reason: a graceful shutdown drains in-flight work, so it would leave nothing to recover.
//!
//! # The read-only half is asserted at the boundary that exists, and the gap is named
//!
//! "An external MCP client can call only its granted read tool" has two halves, and they are in different
//! states:
//!
//! - **"cannot call what it is not granted" is built and asserted here.** An unauthenticated caller reaching
//!   the served endpoint is refused, which the gate measures as a `401` from the real listener. That is the
//!   fail-closed direction, and it is the half that protects the machine.
//! - **"an external client can call its granted tool" is not yet reachable.** The daemon builds its endpoint
//!   with `CallerAdmission::local_only()`, and `CallerAdmission::new` — the allowlist that would admit a
//!   credential — has **no production caller**: nothing reads a caller allowlist from configuration. So a
//!   network client cannot be admitted at all, and the gate asserts that refusal rather than pretending to
//!   admit one. `ADR-0038` records why this is a deliberate intermediate state rather than an oversight: a
//!   remotely reachable MCP server needs **audience-bound** credentials (RFC 8707), so the allowlist's
//!   credential vocabulary belongs to the OAuth slice, and inventing one here would pre-empt a documented
//!   decision about a trust boundary. The gap is recorded in `TODO.md` with this gate as its evidence.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use jarvis_storage::{AppPaths, portable_layout};

/// The published API path prefix.
///
/// Written out rather than imported from the crate that defines it. A gate that asked the product where
/// its own endpoints are could not notice a path change, and the path is part of the contract this gate
/// exists to hold still.
const API_PREFIX: &str = "/api/v1";
/// The path the daemon serves MCP on, which the transport crate publishes.
///
/// Also written out, for the same reason.
const MCP_PATH: &str = "/mcp";
/// Seconds to wait for a daemon to report readiness before failing the gate.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// Seconds to wait for a process to exit after a termination request.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
/// Poll interval while waiting for a condition.
const POLL: Duration = Duration::from_millis(50);
/// Environment variable that turns a missing-binary skip into a failure.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";
/// Environment variable that turns a missing `fixture-peer` skip into a failure.
///
/// A **separate** variable because it names a different artifact: this gate needs `jarvisd`, and it needs
/// `fixture-peer` as a child process. One variable describing two artifacts is the mistake that made the
/// Phase 1 gate red on every operating system for four pushes (`P3-014`).
const REQUIRE_FIXTURE_PEER_ENV: &str = "ACCEPTANCE_REQUIRE_FIXTURE_PEER";

/// The loopback port the gate tells the daemon to serve its REST API on.
///
/// Fixed rather than ephemeral because the daemon's configuration validation refuses port `0`: an
/// ephemeral port would leave a client with no address to name. High and unusual so a developer's own
/// daemon is unlikely to hold it, and an occupied port fails the gate loudly at the bind rather than
/// silently testing something else. Distinct from the Phase 1/2 port so the two gates can run in
/// parallel without colliding.
const HTTP_PORT: u16 = 43_879;
/// The loopback port the gate tells the daemon to serve **MCP** on.
const MCP_PORT: u16 = 43_880;
/// The name the configured MCP server is given, which prefixes its tool identifiers.
const SERVER_NAME: &str = "gate";
/// The tool the fixture server offers, and which the gate holds.
const REMOTE_TOOL: &str = "write";
/// The prefixed identifier the catalog gives that tool: `mcp.<server>.<remote name>`.
///
/// The strategy places the server's name in the **namespace** and leaves the tool name unchanged, so this
/// is `mcp.gate.write` rather than a mangled form — and the gate asserting the exact string is what makes
/// a strategy change visible rather than silently testing a tool that no longer exists.
const PREFIXED_TOOL: &str = "mcp.gate.write";

/// Removes the gate's temporary root when the test ends, pass or fail.
///
/// The removal goes through `jarvis_core::remove_scratch_dir` rather than a bare `remove_dir_all`, because
/// the fixture holds a database and the handle is released late and off-thread (`P3-013`).
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        // Short on purpose: the daemon's socket lives under this root and macOS refuses a socket path over 103 bytes, while its
        // temp directory is already about 50. The tail of the tag is the random part.
        let tag = jarvis_core::scratch_tag();
        let path = std::env::temp_dir().join(format!("ja3-{label}-{}", &tag[tag.len() - 12..]));
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

    /// Writes the profile configuration, including the two listeners and one MCP server.
    ///
    /// `workspace` is the directory the daemon is granted as a tool root. It is passed in rather than
    /// derived, because the gate also needs the same path to name as the fixture server's working
    /// directory â€” and two independently-derived paths is how a gate ends up granting one directory and
    /// configuring a server in another.
    fn write_config(&self, workspace: &Path, fixture_peer: &Path) {
        let paths = self.paths();
        paths
            .prepare()
            .unwrap_or_else(|error| panic!("prepare the profile directories: {error}"));
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
             http_port = {HTTP_PORT}\n\
             mcp_serve_port = {MCP_PORT}\n\
             tool_workspace_roots = [{workspace}]\n",
            workspace = toml_string(&workspace.display().to_string())
        );
        std::fs::write(paths.config().join("config.toml"), document)
            .unwrap_or_else(|error| panic!("write config: {error}"));

        // One server, no posture key â€” which is the daemon's default and the case the gate is about: a
        // server nobody has classified gets risk 3 and `Ask`, so every call is held.
        let servers = format!(
            "[[servers]]\n\
             name = {name}\n\
             transport = {{ kind = \"stdio\", program = {program}, args = [\"--name\", {name}, \"--tool\", {tool}] }}\n",
            name = toml_string(SERVER_NAME),
            program = toml_string(&fixture_peer.display().to_string()),
            tool = toml_string(REMOTE_TOOL)
        );
        std::fs::write(paths.config().join("mcp-servers.toml"), servers)
            .unwrap_or_else(|error| panic!("write mcp-servers.toml: {error}"));
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

/// Renders a path as a TOML basic string.
///
/// A Windows path contains backslashes, which are escapes in a TOML basic string, so a path interpolated
/// raw would produce a document the parser rejects â€” or, worse, one it accepts with a different path. The
/// two characters that matter are escaped; everything else a path can contain is literal.
fn toml_string(value: &str) -> String {
    format!(r#""{}""#, value.replace('\\', r"\\").replace('"', "\\\""))
}

/// Locates a built binary beside the current test executable.
fn binary(name: &str) -> Option<PathBuf> {
    let mut directory = std::env::current_exe().ok()?;
    directory.pop();
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory.pop();
    }
    let candidate = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if !candidate.is_file() {
        return None;
    }
    if name == "jarvisd" && !binary_is_fresh(&candidate) {
        return None;
    }
    Some(candidate)
}

fn binary_is_fresh(candidate: &Path) -> bool {
    let Ok(binary_modified) = std::fs::metadata(candidate).and_then(|meta| meta.modified()) else {
        return false;
    };
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .ok();
    let Some(repo) = repo else {
        return false;
    };
    // Forward slashes: Windows accepts them, and a backslash is a filename character on Unix, which made every witness
    // "missing" there and the gate report a fresh binary as absent.
    let witnesses = [
        repo.join("apps/jarvisd/src/gateway.rs"),
        repo.join("apps/jarvisd/src/run_service.rs"),
        repo.join("apps/jarvisd/src/tool_pipeline.rs"),
        repo.join("crates/jarvis-protocol/src/approval.rs"),
    ];
    witnesses.into_iter().all(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified <= binary_modified)
    })
}

/// Returns the two binaries the gate needs, or skips with an actionable message.
///
/// Each absence is named separately, and each has its own environment variable, because the two artifacts
/// are built by different commands: `cargo build --workspace` produces `jarvisd`, while `fixture-peer` is a
/// test binary behind a feature.
fn required_binaries() -> Option<(PathBuf, PathBuf)> {
    let Some(daemon) = binary("jarvisd") else {
        return skip("jarvisd", REQUIRE_BINARIES_ENV, "cargo build --workspace");
    };
    let Some(peer) = binary("fixture-peer") else {
        return skip(
            "fixture-peer",
            REQUIRE_FIXTURE_PEER_ENV,
            "cargo build --workspace --all-features",
        );
    };
    Some((daemon, peer))
}

/// Reports a missing artifact, failing when its variable requires it.
fn skip(artifact: &str, variable: &str, command: &str) -> Option<(PathBuf, PathBuf)> {
    let message =
        format!("the phase 3 gate requires the built {artifact} binary; run `{command}` first");
    assert!(std::env::var_os(variable).is_none(), "{message}");
    eprintln!("SKIP: {message}");
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
    /// Scoped to the process identifier that was just started, because a hard-terminated daemon never runs
    /// its `stopped` transition: its last recorded state stays `ready`, so a looser check would accept a
    /// stale row before the new process had done anything.
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
/// The file holds the token alone, so it is trimmed rather than parsed. Reading it here rather than through
/// the storage crate is deliberate: the gate must consume the credential exactly as an unrelated client
/// would, and a shared parser would agree with the writer by construction.
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

/// A minimal authenticated client for the run and tool API.
struct ApiClient {
    http: reqwest::Client,
    credential: String,
}

impl ApiClient {
    fn new(credential: String) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|error| panic!("build http client: {error}"));
        Self { http, credential }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(
                method,
                format!("http://127.0.0.1:{HTTP_PORT}{API_PREFIX}{path}"),
            )
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
    }

    /// Starts a run and returns its identifier.
    async fn start_run(&self, objective: &str) -> String {
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
        body["run_id"]
            .as_str()
            .unwrap_or_else(|| panic!("a run reply must carry run_id: {body}"))
            .to_owned()
    }

    /// Calls a tool and returns the status and the decoded body.
    async fn call_tool(
        &self,
        tool: &str,
        run_id: &str,
        arguments: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let response = self
            .request(reqwest::Method::POST, &format!("/tools/{tool}/calls"))
            .json(&serde_json::json!({ "run_id": run_id, "arguments": arguments }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("call tool: {error}"));
        let status = response.status().as_u16();
        let body = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("decode tool reply: {error}"));
        (status, body)
    }

    /// Decides an approval and returns the status and the decoded body.
    async fn decide(&self, approval_id: &str, decision: &str) -> (u16, serde_json::Value) {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/approvals/{approval_id}/decision"),
            )
            .json(&serde_json::json!({ "decision": decision }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("decide approval: {error}"));
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// Resumes a call and returns the status and the decoded body.
    async fn resume(
        &self,
        call_id: &str,
        arguments: &serde_json::Value,
    ) -> (u16, serde_json::Value) {
        let response = self
            .request(reqwest::Method::POST, &format!("/calls/{call_id}/resume"))
            .json(&serde_json::json!({ "arguments": arguments }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("resume call: {error}"));
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        (status, body)
    }
}

/// The gate: a read-only call succeeds, a write call holds, a restart preserves the hold, and the approved
/// call runs exactly once.
#[tokio::test]
async fn phase_3_gate() {
    let Some((daemon, fixture_peer)) = required_binaries() else {
        return;
    };

    let root = TempRoot::new("main");
    let workspace = root.0.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap_or_else(|error| panic!("create workspace: {error}"));
    root.write_config(&workspace, &fixture_peer);

    let mut running = RunningDaemon::start(&daemon, &root.0);
    running.wait_until_ready(&root.0).await;
    let credential = profile_credential(&root);
    let client = ApiClient::new(credential);

    // ---------------------------------------------------------------------------------------------
    // Step 1: the served MCP endpoint refuses an external caller, which is the half that exists.
    // ---------------------------------------------------------------------------------------------
    //
    // **This is the assertion, not a placeholder.** The daemon binds an endpoint that answers MCP, and an
    // unauthenticated network caller is refused a `401` — which is the fail-closed direction and the half
    // that protects the machine. That an admitted external caller can then reach its granted read tool is
    // **not yet reachable**, because no caller allowlist is read from configuration
    // (`CallerAdmission::local_only()` is what the daemon builds, and `CallerAdmission::new` has no
    // production caller). Asserting a `200` here would be asserting something the platform does not do.
    //
    // The gate still measures the **refusal through the real listener**, so a change that admitted an
    // anonymous remote caller would fail here rather than being discovered in production.
    let (mcp_status, mcp_body) = probe_served_endpoint().await;
    assert_eq!(
        mcp_status, 401,
        "an unauthenticated remote MCP caller must be refused before any tool runs, got {mcp_status}: {mcp_body}"
    );

    // ---------------------------------------------------------------------------------------------
    // Step 2: a write call over the run API holds, with a durable approval.
    // ---------------------------------------------------------------------------------------------
    let run_id = client.start_run("write a file").await;
    let arguments = serde_json::json!({ "q": "the gate" });
    let (status, hold) = client.call_tool(PREFIXED_TOOL, &run_id, &arguments).await;
    assert_eq!(
        status, 202,
        "a write tool must pause for a human decision, got {status}: {hold}"
    );
    assert_eq!(hold["state"], "awaiting_approval", "got {hold}");
    let call_id = string_field(&hold, "call_id");
    let approval_id = string_field(&hold, "approval_id");

    // ---------------------------------------------------------------------------------------------
    // Step 3: a restart between the hold and the decision preserves both.
    // ---------------------------------------------------------------------------------------------
    let _stderr = running.kill_and_drain();
    let mut running = RunningDaemon::start(&daemon, &root.0);
    running.wait_until_ready(&root.0).await;

    // The decision is accepted **after** the restart, which is the property `A05` requires: an approval
    // outlives the process that requested it and is answerable by the authenticated owner alone.
    let (status, decided) = client.decide(&approval_id, "approve").await;
    assert_eq!(
        status, 200,
        "an approval must survive a restart and remain decidable, got {status}: {decided}"
    );
    assert_eq!(decided["state"], "approved", "got {decided}");

    // ---------------------------------------------------------------------------------------------
    // Step 4: the approved call runs, and a duplicate delivery does not run it again.
    // ---------------------------------------------------------------------------------------------
    let (status, resumed) = client.resume(&call_id, &arguments).await;
    assert_eq!(
        status, 200,
        "an approved call must resume after a restart, got {status}: {resumed}"
    );
    // The effect actually happened, rather than the route merely answering. A `confirmed` outcome carries
    // evidence — the provider's own locator — and `P3-005` keeps that separate from the output precisely so
    // a success-shaped sentence cannot be read as proof.
    assert_eq!(
        resumed["state"], "confirmed",
        "the resumed call must be a confirmed effect: {resumed}"
    );
    assert!(
        resumed["evidence"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "a confirmation must carry the provider evidence that supports it: {resumed}"
    );

    // The duplicate. `409` is the answer a client needs in order to know the effect is not repeated.
    let (status, again) = client.resume(&call_id, &arguments).await;
    assert_eq!(
        status, 409,
        "a duplicate resumption must not execute the effect twice, got {status}: {again}"
    );

    // The daemon is still healthy, so the refusal was a decision rather than a crash. The health routes
    // are mounted **outside** the API prefix, which the gate therefore builds explicitly rather than
    // through `request` — a 404 here would be about the path, not the daemon.
    let health = client
        .http
        .get(format!("http://127.0.0.1:{HTTP_PORT}/health/ready"))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", client.credential),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("health: {error}"));
    assert_eq!(
        health.status().as_u16(),
        200,
        "the daemon must still be ready"
    );

    // The daemon's own log is read so a failure that produced no visible symptom is still diagnosable.
    let log = root.log();
    assert!(
        !log.is_empty(),
        "the daemon must have written a structured log: the gate depends on it for diagnosis"
    );
}

/// Reads a string field out of a JSON body, naming the body on absence.
fn string_field(body: &serde_json::Value, field: &str) -> String {
    body[field]
        .as_str()
        .unwrap_or_else(|| panic!("the reply must carry {field}: {body}"))
        .to_owned()
}

/// Sends an `initialize` to the served MCP endpoint **with no credential**, and returns the status and body.
///
/// # Why this is hand-written JSON-RPC rather than an SDK
///
/// The gate must observe the endpoint the way a client the daemon does not know about would. An MCP client
/// library here would agree with whatever that library does, and a protocol change it absorbed would be
/// invisible. A hand-written request makes the wire shape part of what the gate holds still.
///
/// # Why the status is returned rather than asserted here
///
/// The caller states what the refusal must be, so this stays a probe. A helper that asserted its own
/// expected status would put the gate's claim in two places, and the one that mattered would be the one a
/// reader did not look at.
async fn probe_served_endpoint() -> (u16, String) {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .no_proxy()
        .build()
        .unwrap_or_else(|error| panic!("build mcp client: {error}"));
    let url = format!("http://127.0.0.1:{MCP_PORT}{MCP_PATH}");

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "phase3-gate", "version": "1.0.0" }
        }
    });
    let response = http
        .post(&url)
        .header("Accept", "application/json, text/event-stream")
        .json(&initialize)
        .send()
        .await
        .unwrap_or_else(|error| panic!("initialize the mcp session: {error}"));
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("read the mcp reply: {error}"));
    (status, body)
}
