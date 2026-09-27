//! Phase 4 acceptance gate: A08 (memory lifecycle) and A09 (workspace isolation), executed as tests.
//!
//! `docs/quality/acceptance-tests.md` states the two claims this gate holds still:
//!
//! - **A08** — "Given the user says 'Remember that client proposals should be concise,' **after restart**
//!   JARVIS retrieves that preference with source and confidence. When corrected, old content is not
//!   current. When forgotten, canonical text, embeddings, indexes, caches, and relationship edges are
//!   deleted according to policy."
//! - **A09** — "Given semantically similar memories/documents in two workspaces, **no** query, context
//!   build, model call, tool, search, export, trace, or diagnostics request from one workspace reveals the
//!   other's content or existence."
//!
//! This is a **process-level** gate, like the three phase gates before it: it runs the real `jarvisd` against
//! a disposable portable root and drives it over its published HTTP API. The library suites prove crates;
//! this proves an installation. No provider is contacted and no credential is invented — the only secret used
//! is the one the daemon issues itself — so the whole gate runs offline.
//!
//! # Why two tests on two ports rather than one
//!
//! Each case needs its own disposable profile, and cargo runs integration tests in parallel. Two daemons
//! contending for one port would fail intermittently, and an intermittently failing gate is a gate that gets
//! disabled. So each test owns a root and a port and cannot disturb the other.
//!
//! # What the gate asserts, and the parts of A08 and A09 it deliberately cannot
//!
//! Three parts of the two cases are **not reachable through any product surface**, so the gate says so
//! rather than asserting something the platform does not do. Each is recorded in `TODO.md` with this gate as
//! its evidence.
//!
//! 1. **Nothing creates an entity over the API.** `POST /api/v1/memories` requires at least one `entity_ids`
//!    value and refuses an empty list (`ADR-0050`), and `memory_entities` is a foreign key — but no route or
//!    CLI verb creates an entity. So the gate calls `jarvis_storage::record_entity`, the product's own
//!    repository function, which is how `apps/jarvisd`'s unit tests do it too. The gate then **asserts the
//!    refusal** for an entity the workspace does not have, so the gap is measured rather than hidden: a user
//!    of the shipped product cannot record a memory, because they have no way to name a subject.
//! 2. **Nothing creates a second workspace.** A profile has exactly one, seeded by migration `0005`. A09 needs
//!    two, so `P4-010` added `jarvis_storage::record_workspace` — a real repository function with its own
//!    tests — and the gate uses it. It is **not** a multi-tenant surface: no session, no credential, no
//!    route, and no actor can name a workspace.
//! 3. **Embeddings are not written by anything and there is no `memory_embeddings` table**, so A08's
//!    "embeddings, indexes, caches" half is asserted only for the parts that exist: the canonical text, the
//!    derived search key, the entity links, the supersession link, and the tombstone. pgvector is `P4-009`'s
//!    remaining work and the gap is recorded there.
//!
//! Also deliberately not asserted here: **the model's context**. A08 says JARVIS "retrieves" the preference,
//! and this gate asserts retrieval through the memory read and search surface, which is the document's
//! user-facing retrieval. `P4-007`'s prompt path needs a configured executor model and a provider, and it is
//! covered by the daemon's own tests rather than by a process gate that would need a model to run.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use jarvis_storage::{AppPaths, portable_layout};

/// The published API path prefix.
///
/// Written out rather than imported from the crate that defines it. A gate that asked the product where its
/// own endpoints are could not notice a path change, and the path is part of the contract this gate holds
/// still.
const API_PREFIX: &str = "/api/v1";
/// Seconds to wait for a daemon to report readiness before failing the gate.
const READY_DEADLINE: Duration = Duration::from_secs(30);
/// Seconds to wait for a process to exit after a termination request.
const STOP_DEADLINE: Duration = Duration::from_secs(20);
/// Poll interval while waiting for a condition.
const POLL: Duration = Duration::from_millis(50);
/// Environment variable that turns a missing-binary skip into a failure.
const REQUIRE_BINARIES_ENV: &str = "ACCEPTANCE_REQUIRE_BINARIES";
/// The loopback port the A08 case's daemon serves on.
///
/// Fixed rather than ephemeral because the daemon's configuration validation refuses port `0`. Distinct from
/// every earlier phase gate's port so the gates can run in parallel, and distinct per test here so the two
/// cases cannot collide.
const A08_HTTP_PORT: u16 = 43_885;
/// The loopback port the A09 case's daemon serves on.
const A09_HTTP_PORT: u16 = 43_886;
/// The claim A08 records, phrased as the acceptance document phrases it.
const PREFERENCE: &str = "client proposals should be concise";
/// The claim A08 corrects it to.
const CORRECTED: &str = "client proposals should be detailed";

/// Removes a gate's temporary root when the test ends, pass or fail.
///
/// The removal goes through `jarvis_core::remove_scratch_dir` rather than a bare `remove_dir_all`, because
/// the fixture holds a database and the handle is released late and off-thread (`P3-013`).
struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "jarvis-phase4-{label}-{}",
            jarvis_core::scratch_tag()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
        Self(path)
    }

    fn paths(&self) -> AppPaths {
        portable_layout(&self.0).unwrap_or_else(|| panic!("portable root must be absolute"))
    }

    /// Returns the absolute path of the profile's database.
    fn database_path(&self) -> PathBuf {
        self.paths()
            .data()
            .join(jarvis_storage::DEFAULT_DATABASE_FILENAME)
    }

    /// Returns the daemon's structured log.
    fn log(&self) -> String {
        std::fs::read_to_string(self.paths().logs().join("jarvisd.jsonl")).unwrap_or_default()
    }

    /// Writes the profile configuration: the loopback HTTP listener, enabled, on the given port.
    ///
    /// The REST API is what the memory surface is reachable through, so unlike the earlier gates this one
    /// needs no MCP server and no granted tool root. Nothing else is configured, because a gate that switched
    /// on features the claim does not need would be measuring those features too.
    fn write_config(&self, http_port: u16) {
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
             http_port = {http_port}\n"
        );
        std::fs::write(paths.config().join("config.toml"), document)
            .unwrap_or_else(|error| panic!("write config: {error}"));
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
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

/// Returns the daemon binary, or skips with an actionable message.
fn required_daemon() -> Option<PathBuf> {
    let Some(daemon) = binary("jarvisd") else {
        let message =
            "the phase 4 gate requires the built jarvisd binary; run `cargo build --workspace`";
        assert!(
            std::env::var_os(REQUIRE_BINARIES_ENV).is_none(),
            "{message}"
        );
        eprintln!("SKIP: {message}");
        return None;
    };
    Some(daemon)
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
    async fn wait_until_ready(&mut self, root: &TempRoot) {
        let database = root.database_path();
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

    /// Kills the process abruptly, so a restart leaves whatever the daemon had already made durable.
    ///
    /// Abrupt rather than graceful on purpose: a graceful shutdown drains what was in flight, so it would
    /// leave nothing to prove. A08's "after restart" is only meaningful if the restart is the hard kind.
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

/// A minimal authenticated client for the memory API.
struct ApiClient {
    http: reqwest::Client,
    credential: String,
    port: u16,
}

impl ApiClient {
    fn new(credential: String, port: u16) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|error| panic!("build http client: {error}"));
        Self {
            http,
            credential,
            port,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{API_PREFIX}{path}", self.port)
    }

    /// Issues an authenticated request with a JSON body, returning the status and the decoded reply.
    ///
    /// Raw JSON rather than the product's own DTOs, because part of what the gate holds still is the **wire
    /// shape**: a client built from the daemon's types would follow a field rename without noticing, and a
    /// rename is exactly the kind of change an acceptance gate exists to catch.
    async fn post_json(&self, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
        let response = self
            .http
            .request(reqwest::Method::POST, self.url(path))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
            .json(body)
            .send()
            .await
            .unwrap_or_else(|error| panic!("POST {path}: {error}"));
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// Issues an authenticated `GET` and returns the status and the decoded reply.
    async fn get(&self, path: &str) -> (u16, serde_json::Value) {
        let response = self
            .http
            .get(self.url(path))
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.credential),
            )
            .send()
            .await
            .unwrap_or_else(|error| panic!("GET {path}: {error}"));
        let status = response.status().as_u16();
        let body = response.json().await.unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    /// Issues an **unauthenticated** `GET`, which is how the fail-closed direction is measured.
    async fn get_unauthenticated(&self, path: &str) -> u16 {
        self.http
            .get(self.url(path))
            .send()
            .await
            .unwrap_or_else(|error| panic!("unauthenticated GET {path}: {error}"))
            .status()
            .as_u16()
    }
}

/// A remember body naming one entity.
///
/// `entity_ids` is present because `ADR-0050` refuses an entity-less remember rather than filing a claim
/// against a placeholder subject — which is the reason this gate has to create an entity at all.
fn remember_body(entity_id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "content": content,
        "memory_type": "preference",
        "source_kind": "user_statement",
        "entity_ids": [entity_id],
    })
}

/// Returns the workspace the profile's seeded identity names.
async fn local_workspace(database: &jarvis_storage::SqliteDatabase) -> jarvis_core::WorkspaceId {
    let identity = jarvis_storage::load_local_identity(database)
        .await
        .unwrap_or_else(|error| panic!("the profile must have a seeded identity: {error}"));
    identity
        .workspace_id()
        .parse()
        .unwrap_or_else(|_| panic!("the seeded workspace identifier must parse"))
}

/// Records an entity in a workspace, returning its identifier.
///
/// # Why this is the storage crate and not the API
///
/// There is **no product surface that creates an entity** — no route, no CLI verb, no configuration. So the
/// gate reaches the product's own repository function, which is what `apps/jarvisd`'s unit tests do. The gate
/// asserts the resulting refusal separately, so this is not hiding the gap: it is the only way to create the
/// precondition A08 needs, and the assertion for an unknown entity proves the API genuinely cannot.
async fn record_gate_entity(
    database: &jarvis_storage::SqliteDatabase,
    workspace_id: jarvis_core::WorkspaceId,
) -> jarvis_core::EntityId {
    let entity_id = jarvis_core::EntityId::new();
    jarvis_storage::record_entity(
        database,
        &jarvis_storage::NewEntity {
            id: entity_id,
            workspace_id,
            kind: jarvis_storage::EntityKind::Person,
            label: "Gate subject".to_owned(),
            attributes: None,
            confidence: jarvis_core::MemoryConfidence::Confirmed,
            created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
        },
    )
    .await
    .unwrap_or_else(|error| panic!("record the gate entity: {error}"));
    entity_id
}

/// Records a claim in a workspace through the real write path, returning its identifier.
///
/// # Why the product's own `record_memory` rather than raw SQL
///
/// A fixture that wrote its own `INSERT` would be asserting that a hand-written row is isolated — a claim
/// about SQLite rather than about this platform. `record_memory` is the function `apps/jarvisd` calls, so the
/// foreign claim A09 hides is **exactly** the row the product would have stored, including its derived search
/// key and its entity link.
///
/// The entity must already exist, because `memory_entities` is a foreign key: the schema keeps "a memory is
/// about something" true rather than leaving it to a convention.
async fn record_gate_claim(
    database: &jarvis_storage::SqliteDatabase,
    workspace: jarvis_core::WorkspaceId,
    entity: jarvis_core::EntityId,
    content: &str,
) -> jarvis_core::MemoryId {
    let record = jarvis_core::MemoryRecord::new(jarvis_core::MemoryRecordParts {
        id: jarvis_core::MemoryId::new(),
        workspace_id: workspace,
        memory_type: jarvis_core::MemoryType::Preference,
        content: content.to_owned(),
        structured_claim: None,
        // `user_statement` is authoritative by the schema's own equality check, and `confirmed` is the
        // confidence a user statement supports — the same pairing the API stores.
        source: jarvis_core::MemorySource::of_kind(
            jarvis_core::MemorySourceKind::UserStatement,
            "gate:fixture",
        )
        .unwrap_or_else(|error| panic!("build the gate source: {error}")),
        confidence: jarvis_core::MemoryConfidence::Confirmed,
        importance: 2,
        sensitivity: jarvis_core::Sensitivity::Internal,
        entities: vec![jarvis_core::EntityRef::confirmed(entity)],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: "gate".to_owned(),
        correlation_id: jarvis_core::CorrelationId::new(),
        created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
    })
    .unwrap_or_else(|error| panic!("build the gate record: {error}"));

    // The key is derived by the **domain**, not written by hand. The unique index is
    // `(workspace_id, search_key)`, so a hand-built key is what would let two workspaces' claims collide — or
    // fail to — for a reason the test could not see.
    let key = jarvis_core::MemorySearchKey::new(
        record.memory_type(),
        record.entities(),
        record.content(),
    )
    .unwrap_or_else(|error| panic!("derive the gate search key: {error}"));

    let id = jarvis_storage::record_memory(database, &record, &key)
        .await
        .unwrap_or_else(|error| panic!("record the gate claim: {error}"));
    assert_eq!(
        id,
        record.id().to_string(),
        "the stored claim must be the one the fixture built"
    );
    record.id()
}

/// Ids of every entry in a listing, search, or export reply.
///
/// One helper for all three shapes because the field name is the same and the assertion is the same: which
/// identifiers did this workspace's read offer. A helper per shape would be three places to keep in step.
fn matched_ids(reply: &serde_json::Value, field: &str) -> Vec<String> {
    reply[field]
        .as_array()
        .unwrap_or_else(|| panic!("a reply must carry a {field} array: {reply}"))
        .iter()
        .map(|entry| {
            entry["memory_id"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry must carry a memory_id: {entry}"))
                .to_owned()
        })
        .collect()
}

/// Reads a string field out of a JSON body, naming the body on absence.
fn string_field(body: &serde_json::Value, field: &str) -> String {
    body[field]
        .as_str()
        .unwrap_or_else(|| panic!("the reply must carry {field}: {body}"))
        .to_owned()
}

/// Ids of the hits in a search reply that **actually matched** the query.
///
/// # Why `is_a_match` and not "the reply was empty"
///
/// `ADR-0046`'s ranking legitimately includes a recent claim that matched nothing, and the domain exposes
/// `is_a_match` precisely so a memory that answered no question cannot be presented as an answer. A test of
/// "did this get stored" therefore has to ask whether anything *matched*, not whether anything *came back* —
/// this gate's first version asserted an empty result and failed against a correct implementation.
fn matching_hits(reply: &serde_json::Value) -> Vec<String> {
    reply["matches"]
        .as_array()
        .unwrap_or_else(|| panic!("a search must carry a matches array: {reply}"))
        .iter()
        .filter(|hit| hit["is_a_match"] == true)
        .map(|hit| {
            hit["memory_id"]
                .as_str()
                .unwrap_or_else(|| panic!("every hit must carry a memory_id: {hit}"))
                .to_owned()
        })
        .collect()
}

/// **A08: a claim about a subject the workspace does not have is refused, and nothing is invented.**
///
/// This is the larger half of `P4-010`'s finding, and it is the gate's job to keep it measured. The service
/// used to **create** any entity a remember named, so this request was answered `201` for a claim about a
/// fabricated subject.
///
/// # Why the wording is distinct from any stored claim's
///
/// With the same words as a stored claim, the request would be refused by the **unique index** whatever the
/// entity check did — and this assertion would hold against the very defect it exists to catch. Distinct
/// wording is part of the test rather than incidental, and it is how the defect was found.
async fn a08_an_unknown_subject_is_refused(client: &ApiClient) {
    let wording = "a claim about a subject that was never established";
    let (status, refused) = client
        .post_json(
            "/memories",
            &remember_body(&jarvis_core::EntityId::new().to_string(), wording),
        )
        .await;
    assert_eq!(
        status, 422,
        "a claim about an entity the workspace does not have must be refused by field, got {status}: {refused}"
    );

    // **The fabrication is also absent from the claim store, and that is an independent fact.** A check that
    // stopped at the refusal would pass against a service that invented the subject *and* stored the claim
    // under a later identifier — the refusal would then be about something else entirely.
    let (status, ghost) = client
        .post_json("/memories/search", &serde_json::json!({ "text": wording }))
        .await;
    assert_eq!(status, 200, "the search must answer: {ghost}");
    assert_eq!(
        matching_hits(&ghost).len(),
        0,
        "the refused request must not have stored a claim under a fabricated subject: {ghost}"
    );
}

/// Reads an integer field out of a JSON body, naming the body on absence.
fn integer_field(body: &serde_json::Value, field: &str) -> i64 {
    body[field]
        .as_i64()
        .unwrap_or_else(|| panic!("the reply must carry {field}: {body}"))
}

/// **A08: the memory lifecycle.** Remember, survive a restart, correct, and forget.
///
/// Every step is asserted against a *subsequent* request rather than against the reply that caused it,
/// because a handler that returned the right shape and wrote nothing would satisfy a reply-only assertion.
/// The restart is the step that makes the persistence claim real, and the correction and deletion steps each
/// assert that the **old** claim stopped being current rather than merely that a new one appeared.
#[tokio::test]
async fn phase_4_gate_a08_memory_lifecycle() {
    let Some(daemon) = required_daemon() else {
        return;
    };
    let root = TempRoot::new("a08");
    root.write_config(A08_HTTP_PORT);

    // The entity is created before the daemon serves anything, through the storage crate rather than the API,
    // because no surface creates one. A08's precondition needs a subject.
    let entity_id = {
        let database = jarvis_storage::SqliteDatabase::open(&root.database_path())
            .await
            .unwrap_or_else(|error| panic!("open the gate database: {error}"));
        let workspace = local_workspace(&database).await;
        let entity_id = record_gate_entity(&database, workspace).await;
        database.close().await;
        entity_id.to_string()
    };

    let mut running = RunningDaemon::start(&daemon, &root.0);
    running.wait_until_ready(&root).await;
    let client = ApiClient::new(profile_credential(&root), A08_HTTP_PORT);

    // ---------------------------------------------------------------------------------------------
    // Step 1: the memory surface refuses an unauthenticated caller.
    // ---------------------------------------------------------------------------------------------
    //
    // Asserted first so a later failure cannot be mistaken for an authentication problem, and asserted at all
    // because a memory surface that answered an anonymous caller is the worst defect this gate could miss.
    assert_eq!(
        client.get_unauthenticated("/memories").await,
        401,
        "the memory surface must refuse an unauthenticated caller"
    );

    // ---------------------------------------------------------------------------------------------
    // Step 2: the preference is recorded.
    // ---------------------------------------------------------------------------------------------
    let body = remember_body(&entity_id, PREFERENCE);
    let (status, reply) = client.post_json("/memories", &body).await;
    assert_eq!(
        status, 201,
        "a new claim must be created, got {status}: {reply}"
    );
    assert_eq!(
        reply["outcome"], "remembered",
        "a new claim must report itself as stored: {reply}"
    );
    let memory_id = string_field(&reply, "memory_id");
    let version = integer_field(&reply, "version");
    assert_eq!(version, 1, "a new claim's first version is one");
    a08_an_unknown_subject_is_refused(&client).await;

    // ---------------------------------------------------------------------------------------------
    // Step 3: **after a restart**, the preference is retrieved with its source and confidence.
    // ---------------------------------------------------------------------------------------------
    let _stderr = running.kill_and_drain();
    let mut running = RunningDaemon::start(&daemon, &root.0);
    running.wait_until_ready(&root).await;
    a08_read_after_restart(&client, &memory_id).await;

    // ---------------------------------------------------------------------------------------------
    // Step 4: after a correction, the old content is **not current**.
    // ---------------------------------------------------------------------------------------------
    //
    // The step returns the replacement and its version, because step 5 operates on the replacement: a correction
    // is a new claim, so the identifier the caller must forget is the one the correction produced.
    let (replacement_id, replacement_version) = a08_correct(&client, &memory_id, version).await;

    // ---------------------------------------------------------------------------------------------
    // Step 5: after a forget, the canonical text, the search key, the entity links, and the trail are gone.
    // ---------------------------------------------------------------------------------------------
    let (status, receipt) = client
        .post_json(
            &format!("/memories/{replacement_id}/forget"),
            &serde_json::json!({ "expected_version": replacement_version }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a forget must be accepted, got {status}: {receipt}"
    );
    let removed = u32::try_from(integer_field(&receipt, "removed_content_chars"))
        .unwrap_or_else(|_| panic!("a character count must be non-negative: {receipt}"));
    assert_eq!(
        removed,
        u32::try_from(CORRECTED.chars().count()).unwrap_or(u32::MAX),
        "the receipt must account for the text it removed: {receipt}"
    );
    assert_eq!(
        receipt["removed_search_key"], true,
        "the derived key holds the claim's words, so it must go with them: {receipt}"
    );
    assert_eq!(
        integer_field(&receipt, "removed_entity_links"),
        1,
        "the claim's entity link must be removed and counted: {receipt}"
    );
    assert_eq!(
        receipt["tombstone_written"], true,
        "a deletion must leave a tombstone so the claim cannot return by accident: {receipt}"
    );
    // A08 says "deleted according to policy", and the policy's own statement of what it cannot reach is the
    // receipt's `unreachable` list. A receipt that read as total would be misleading at the moment it is
    // supposed to inform.
    let unreachable = receipt["unreachable"]
        .as_array()
        .unwrap_or_else(|| panic!("a receipt must carry an unreachable list: {receipt}"));
    assert!(
        !unreachable.is_empty(),
        "a deletion receipt must name what it could not reach: {receipt}"
    );
    a08_purged_is_unreachable(&client, &entity_id, &replacement_id).await;

    assert!(
        !root.log().is_empty(),
        "the daemon must have written a structured log: the gate depends on it for diagnosis"
    );
}

/// **A08: after a restart, the claim is retrieved with its source and confidence.**
///
/// The restart itself happens in the caller, which owns the daemon handle; this asserts what the *restarted*
/// daemon answers. That is the acceptance document's own word — "**after restart** JARVIS retrieves that
/// preference with source and confidence" — and the restart is a hard kill, so what is read here is what had
/// already been made durable rather than what a graceful drain had flushed.
async fn a08_read_after_restart(client: &ApiClient, memory_id: &str) {
    let (status, listing) = client.get("/memories?limit=50").await;
    assert_eq!(
        status, 200,
        "the listing must answer, got {status}: {listing}"
    );
    assert!(
        matched_ids(&listing, "memories").contains(&memory_id.to_owned()),
        "the claim must survive a restart: {listing}"
    );

    let (status, detail) = client.get(&format!("/memories/{memory_id}")).await;
    assert_eq!(status, 200, "the claim must be readable: {detail}");
    assert_eq!(
        detail["content"], PREFERENCE,
        "the retrieved claim must be the text that was stored: {detail}"
    );
    // "with source and confidence" is the document's phrase, so both are asserted rather than assumed to ride
    // along on the content. A claim that came back with its text but not its provenance would satisfy "the
    // memory is still there" while failing the acceptance case.
    assert_eq!(
        detail["source_kind"], "user_statement",
        "the retrieved claim must carry its source: {detail}"
    );
    assert_eq!(
        detail["confidence"], "confirmed",
        "a user statement's confidence must survive the round trip: {detail}"
    );
    assert_eq!(
        detail["source_locator"], "api:memories",
        "the source must name where the claim came from: {detail}"
    );
}

/// **A08: a correction replaces the claim, and the old content stops being current.**
///
/// Returns the replacement's identifier and version, because a correction **is a new claim** — so the value the
/// caller must forget next is the one the correction produced, not the original's.
///
/// The three assertions are three different facts, and each is separately achievable while the others fail: the
/// correction is accepted; the **old** claim is no longer offered by retrieval; and the superseded row stays
/// **readable by identifier** so the trail is auditable. The last is what stops "not current" being satisfied
/// by "deleted" (`ADR-0050` keeps the row precisely so a user can see what changed).
async fn a08_correct(client: &ApiClient, memory_id: &str, version: i64) -> (String, i64) {
    let (status, corrected) = client
        .post_json(
            &format!("/memories/{memory_id}/correct"),
            &serde_json::json!({ "content": CORRECTED, "expected_version": version }),
        )
        .await;
    assert_eq!(
        status, 200,
        "a correction must be accepted, got {status}: {corrected}"
    );
    assert_eq!(corrected["outcome"], "corrected", "got {corrected}");
    let replacement_id = string_field(&corrected, "memory_id");
    assert_ne!(
        replacement_id, memory_id,
        "a correction is a new claim with a supersedes link, never an in-place overwrite"
    );

    // "Old content is not current" is a claim about **retrieval**, so it is asserted through the search surface
    // rather than by reading the row's status: status is the mechanism, and a test that asserted the mechanism
    // would pass while retrieval still offered the stale claim.
    let (status, found) = client
        .post_json(
            "/memories/search",
            &serde_json::json!({ "text": PREFERENCE }),
        )
        .await;
    assert_eq!(status, 200, "the search must answer: {found}");
    let matched = matched_ids(&found, "matches");
    assert!(
        !matched.contains(&memory_id.to_owned()),
        "a superseded claim must not be offered as current: {found}"
    );
    assert!(
        matched.contains(&replacement_id),
        "the corrected claim is what answers the original words: {found}"
    );

    a08_correct_keeps_the_trail(client, memory_id, &replacement_id).await;
    (replacement_id, integer_field(&corrected, "version"))
}

/// **A08: the superseded row stays readable, and the reply's wire shape is flat.**
///
/// # Why the fields are read at the top level
///
/// `MemoryDetailReply` **flattens** its reference, so the wire shape is flat and a nested read returns `null`.
/// This gate's first version asserted `superseded["reference"]["status"]` and failed against a `null` — the gate
/// holding the wire shape still exactly as intended, since a client written against the old nesting would have
/// broken in production rather than here.
async fn a08_correct_keeps_the_trail(client: &ApiClient, memory_id: &str, replacement_id: &str) {
    let (status, superseded) = client.get(&format!("/memories/{memory_id}")).await;
    assert_eq!(
        status, 200,
        "a superseded claim must remain readable for the audit trail: {superseded}"
    );
    assert_eq!(
        superseded["status"], "archived",
        "the superseded claim must not still read as active: {superseded}"
    );
    assert_eq!(
        superseded["effective_status"], "superseded",
        "and the effective status must say so, which is the value a presentation site asks about: {superseded}"
    );
    assert_eq!(
        superseded["superseded_by"], replacement_id,
        "the superseded claim must name its replacement so the trail is followable: {superseded}"
    );
    assert_eq!(
        superseded["is_stated_as_fact"], false,
        "a superseded claim must not be offered as established: {superseded}"
    );
    assert_eq!(
        superseded["content"], PREFERENCE,
        "the superseded text is retained, because the correction is an audit trail: {superseded}"
    );
}

/// **A08: a purged claim is gone from every content-bearing surface and can never return.**
///
/// Four separate facts, because "deleted" is four claims at this surface: the text is unreadable, a search does
/// not retrieve it, a re-ingest of the same words does not resurrect it, and the export does not carry it.
async fn a08_purged_is_unreachable(client: &ApiClient, entity_id: &str, replacement_id: &str) {
    // The text is gone from the surfaces that carry content, which is the observable form of "canonical text is
    // deleted" rather than an assertion that a particular row vanished.
    let (status, after) = client.get(&format!("/memories/{replacement_id}")).await;
    assert_eq!(
        status, 404,
        "a purged claim must not be readable, got {status}: {after}"
    );
    let (status, found) = client
        .post_json(
            "/memories/search",
            &serde_json::json!({ "text": CORRECTED }),
        )
        .await;
    assert_eq!(status, 200, "the search must still answer: {found}");
    assert!(
        !matched_ids(&found, "matches").contains(&replacement_id.to_owned()),
        "a forgotten claim must not be retrievable: {found}"
    );

    // The claim cannot be learned again, which is what the tombstone is for: the same words at the same key must
    // be refused rather than resurrected by a later ingest.
    let (status, relearned) = client
        .post_json("/memories", &remember_body(entity_id, CORRECTED))
        .await;
    assert_ne!(
        status, 201,
        "a forgotten claim must not be re-learned by an ordinary ingest: {relearned}"
    );

    // The export is the **full user read**, and it is where a purge is still observable — as an absence.
    let (status, export) = client.get("/memories/export?limit=200").await;
    assert_eq!(status, 200, "the export must answer: {export}");
    assert!(
        !matched_ids(&export, "memories").contains(&replacement_id.to_owned()),
        "a purged claim must be absent from the export, not present as a tombstone: {export}"
    );
}

/// **A09: workspace isolation.** A claim in another workspace is not revealed by any read.
///
/// The other workspace's claim is deliberately **word-for-word identical** to this workspace's, because that
/// is the claim: semantically similar content in two workspaces must not reveal the other's content *or*
/// existence. Two differently-worded claims would pass a keyword filter with no isolation at all, which is the
/// weakness this fixture exists to avoid.
#[tokio::test]
async fn phase_4_gate_a09_workspace_isolation() {
    let Some(daemon) = required_daemon() else {
        return;
    };
    let root = TempRoot::new("a09");
    root.write_config(A09_HTTP_PORT);

    let fixture = a09_fixture(&root).await;
    let (local_id, foreign_id) = (fixture.local_id.clone(), fixture.foreign_id.clone());

    // The fixture is real: both claims exist, with identical text, in the database the daemon serves. Without
    // this the gate could pass by having nothing to leak.
    assert_ne!(
        local_id, foreign_id,
        "the fixture must produce two distinct claims, or the isolation assertions are vacuous"
    );

    let mut running = RunningDaemon::start(&daemon, &root.0);
    running.wait_until_ready(&root).await;
    let client = ApiClient::new(profile_credential(&root), A09_HTTP_PORT);

    // ---------------------------------------------------------------------------------------------
    // The listing: the read that would leak by *presence*.
    // ---------------------------------------------------------------------------------------------
    let (status, listing) = client.get("/memories?limit=100").await;
    assert_eq!(status, 200, "the listing must answer: {listing}");
    let listed = matched_ids(&listing, "memories");
    assert!(
        listed.contains(&local_id),
        "this workspace's own claim must be listed: {listing}"
    );
    assert!(
        !listed.contains(&foreign_id),
        "a listing must not reveal another workspace's claim: {listing}"
    );

    // ---------------------------------------------------------------------------------------------
    // The search: the read that could leak through a *ranking*.
    // ---------------------------------------------------------------------------------------------
    //
    // A query that scored the foreign claim would change the local claim's placement even if the foreign row
    // were filtered out afterwards. So this asserts the foreign identifier is absent from the ranked hits
    // *and* that the candidate count saw only one claim — which is where such a leak would otherwise be
    // invisible.
    let (status, found) = client
        .post_json(
            "/memories/search",
            &serde_json::json!({ "text": PREFERENCE }),
        )
        .await;
    assert_eq!(status, 200, "the search must answer: {found}");
    let matched = matched_ids(&found, "matches");
    assert!(
        matched.contains(&local_id),
        "the search must find this workspace's own claim: {found}"
    );
    assert!(
        !matched.contains(&foreign_id),
        "a search must not reveal another workspace's claim: {found}"
    );
    assert_eq!(
        integer_field(&found, "considered"),
        1,
        "the ranking must consider only this workspace's claims: {found}"
    );

    // ---------------------------------------------------------------------------------------------
    // The write that could cross the boundary: a claim naming **another workspace's entity**.
    // ---------------------------------------------------------------------------------------------
    //
    // This is the direction the read probes cannot reach. A remember names its subject over the wire, and an
    // entity identifier is not scoped by `deny_unknown_fields` — so the service has to check that the subject
    // belongs to the caller's own workspace. Without that check a caller can **write** a claim into its own
    // workspace that points at another workspace's subject, which is a cross-workspace link dressed as a local
    // claim: the entity's owner would see a claim about their subject in a workspace they cannot read, and the
    // identity vocabulary would be shared across a boundary every read respects.
    //
    // The wording is distinct from the stored claim's on purpose, so a refusal cannot come from the duplicate
    // index instead of the scope check.
    a09_cross_boundary_write_is_refused(&client, &fixture.foreign_entity_id).await;

    // ---------------------------------------------------------------------------------------------
    // The read by identifier, and the write: the direct probes.
    // ---------------------------------------------------------------------------------------------
    let (status, foreign) = client.get(&format!("/memories/{foreign_id}")).await;
    assert_eq!(
        status, 404,
        "a claim in another workspace must read as absent, never as forbidden — and never as found: {foreign}"
    );

    // The write half of the same boundary. A `404` here means the scope check ran before the version guard
    // could leak whether the claim exists, which is the rule `ADR-0050` records: "not yours" would confirm
    // that something exists.
    let (status, refused) = client
        .post_json(
            &format!("/memories/{foreign_id}/forget"),
            &serde_json::json!({ "expected_version": 1 }),
        )
        .await;
    assert_eq!(
        status, 404,
        "another workspace's claim must not be deletable, got {status}: {refused}"
    );

    // ---------------------------------------------------------------------------------------------
    // The export: the surface with the most to leak, because it returns content for every claim it covers.
    // ---------------------------------------------------------------------------------------------
    //
    // `limit=200` is the daemon's own maximum (`MAX_MEMORY_PAGE`), which this gate learned by being refused a
    // larger page rather than by reading a constant. So it asks for the largest page the product offers instead
    // of a number it invented, and the `422` that taught it is the product's documented bound working.
    let (status, export) = client.get("/memories/export?limit=200").await;
    assert_eq!(status, 200, "the export must answer: {export}");
    let exported = matched_ids(&export, "memories");
    assert!(
        exported.contains(&local_id),
        "the export must contain this workspace's own claim: {export}"
    );
    assert!(
        !exported.contains(&foreign_id),
        "an export must not reveal another workspace's claim: {export}"
    );
    // The export names the workspace it covers, and it must be **this** one. A scope bug that exported
    // everything would otherwise look correct as long as the local claim was present.
    assert_eq!(
        string_field(&export, "workspace_id"),
        profile_workspace(&root).await,
        "the export must name the workspace this profile acts as: {export}"
    );

    // ---------------------------------------------------------------------------------------------
    // The positive direction: this workspace's own claim is still fully readable.
    // ---------------------------------------------------------------------------------------------
    //
    // Without this, every assertion above would pass against a daemon that refused all reads.
    let (status, detail) = client.get(&format!("/memories/{local_id}")).await;
    assert_eq!(
        status, 200,
        "this workspace's own claim must remain readable: {detail}"
    );
    assert_eq!(detail["content"], PREFERENCE, "got {detail}");

    assert!(
        !root.log().is_empty(),
        "the daemon must have written a structured log: the gate depends on it for diagnosis"
    );
}

/// The identifiers an A09 fixture produces.
///
/// The `_id` suffix is shared by every field because every field **is** an identifier — the fixture's whole
/// content is "which three rows exist". Naming them `local`, `foreign`, and `foreign_entity` would be shorter
/// and would read as if they were values of unknown kind, which is the opposite of what a fixture of
/// identifiers should say. So the lint is allowed with its reason rather than the names being degraded.
#[allow(clippy::struct_field_names)]
struct A09Fixture {
    /// This profile's own claim.
    local_id: String,
    /// The other workspace's claim, which must never be revealed.
    foreign_id: String,
    /// The other workspace's entity, which no claim may name across the boundary.
    foreign_entity_id: String,
}

/// Builds two workspaces with **word-for-word identical** claims, through the product's own repository functions.
///
/// # Why the text is identical
///
/// That *is* the claim: semantically similar content in two workspaces must not reveal the other's content or
/// existence. Two differently-worded claims would pass a keyword filter with no isolation at all, which is the
/// weakness this fixture exists to avoid — and a fixture that used different words would be a test of nothing.
///
/// # Why the product's own functions rather than SQL
///
/// A fixture writing its own `INSERT` would be asserting that a hand-written row is isolated — a claim about
/// SQLite rather than about this platform. `record_workspace`, `record_entity`, and `record_memory` are the
/// functions `apps/jarvisd` calls, so the foreign claim is **exactly** the row the product would have stored,
/// derived search key and entity link included.
async fn a09_fixture(root: &TempRoot) -> A09Fixture {
    let database = jarvis_storage::SqliteDatabase::open(&root.database_path())
        .await
        .unwrap_or_else(|error| panic!("open the gate database: {error}"));
    let local_workspace = local_workspace(&database).await;
    let local_entity = record_gate_entity(&database, local_workspace).await;
    let local_id = record_gate_claim(&database, local_workspace, local_entity, PREFERENCE).await;

    // `P4-010` added `record_workspace` for this: no surface creates a second workspace.
    let foreign_workspace = jarvis_core::WorkspaceId::new();
    jarvis_storage::record_workspace(
        &database,
        &jarvis_storage::NewWorkspace {
            id: foreign_workspace,
            name: "Gate foreign workspace".to_owned(),
            mode: jarvis_storage::WorkspaceMode::Local,
            data_policy: jarvis_storage::DataPolicy::Standard,
            created_at: jarvis_core::UtcTimestamp::now(&jarvis_core::SystemClock),
        },
    )
    .await
    .unwrap_or_else(|error| panic!("record the foreign workspace: {error}"));
    let foreign_entity = record_gate_entity(&database, foreign_workspace).await;
    let foreign_id =
        record_gate_claim(&database, foreign_workspace, foreign_entity, PREFERENCE).await;
    database.close().await;

    A09Fixture {
        local_id: local_id.to_string(),
        foreign_id: foreign_id.to_string(),
        foreign_entity_id: foreign_entity.to_string(),
    }
}

/// **A09: a claim naming another workspace's entity is refused, and nothing is written.**
///
/// This is the direction the read probes cannot reach, and the reason `P4-010` added the entity-workspace check
/// to the write path. A remember names its subject over the wire, and an entity identifier is **not** scoped by
/// `deny_unknown_fields` — so the service has to check that the subject belongs to the caller's own workspace.
///
/// Without that check a caller can **write** a claim into its own workspace pointing at another workspace's
/// subject, which is a cross-workspace link dressed as a local claim: the entity's owner would see a claim about
/// their subject in a workspace they cannot read, and the identity vocabulary would be shared across a boundary
/// every read respects.
///
/// The wording is distinct from the stored claim's on purpose, so a refusal cannot come from the duplicate index
/// instead of the scope check.
async fn a09_cross_boundary_write_is_refused(client: &ApiClient, foreign_entity_id: &str) {
    let wording = "a claim filed across the workspace boundary";
    let (status, refused) = client
        .post_json("/memories", &remember_body(foreign_entity_id, wording))
        .await;
    assert_eq!(
        status, 422,
        "a claim naming another workspace's entity must be refused by field, got {status}: {refused}"
    );

    // The claim really was not written, which is a second fact from the refusal: the words must retrieve no
    // **match**.
    let (status, ghost) = client
        .post_json("/memories/search", &serde_json::json!({ "text": wording }))
        .await;
    assert_eq!(status, 200, "the search must answer: {ghost}");
    assert_eq!(
        matching_hits(&ghost).len(),
        0,
        "the refused request must not have stored a claim against another workspace's subject: {ghost}"
    );
}

/// Returns the workspace the profile acts as, read from the profile rather than from a constant.
///
/// A constant would agree with a wrong implementation: the assertion is that the export names what the
/// **profile resolves to**, so the value has to come from the same resolution the daemon performs.
async fn profile_workspace(root: &TempRoot) -> String {
    let database = jarvis_storage::SqliteDatabase::open(&root.database_path())
        .await
        .unwrap_or_else(|error| panic!("open the gate database: {error}"));
    let workspace = local_workspace(&database).await;
    database.close().await;
    workspace.to_string()
}
