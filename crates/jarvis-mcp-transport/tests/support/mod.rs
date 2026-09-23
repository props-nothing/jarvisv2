//! A scripted MCP peer, so the negotiation is driven against the **wire** rather than against this
//! crate's own assumptions.
//!
//! # Why the whole module allows dead code
//!
//! This is a shared test-support module, and Cargo compiles it **once per integration-test binary**.
//! Each binary uses a different subset of it — `wire.rs` needs the scripted replies, `host.rs` and
//! `http.rs` need the fixture builders, `adapter.rs` needs only the discover and tool-list shapes — so a
//! per-item `allow` would have to name, for every helper, the binaries that happen not to use it. That
//! list would go stale on the next test file added, and the failure mode is a build error in an unrelated
//! test.
//!
//! The alternative — deleting what one binary does not use — is worse: the helpers exist precisely
//! because several suites need them, and removing one because a single binary stopped referring to it
//! would delete another suite's fixture. So the allowance is module-wide and says so, which is honest
//! about what it covers rather than hiding it item by item.
#![allow(
    dead_code,
    reason = "shared test support compiled once per test binary; each uses a subset"
)]
//!
//! # Why not the SDK's server
//!
//! The SDK ships a server, and using it would be less code. It would also be worthless as evidence
//! here. `P3-007` and `docs/development/external-research.md` both record the same lesson from a
//! different transport: a fixture built from the same library as the code under test shares its
//! assumptions, so a passing test proves self-consistency and not conformance. If the SDK's client
//! and server disagreed with the *specification* in the same way, an SDK-server test would pass.
//!
//! So this peer writes and reads the wire format directly — newline-delimited JSON-RPC 2.0, which is
//! what `rmcp`'s stdio framing uses (verified in `transport/async_rw.rs`: `read_until(b'\n', ..)` and
//! `put_u8(b'\n')`). If the SDK's framing were something else, every test here would fail rather
//! than pass, which is the property that makes the peer worth writing.
//!
//! # What it deliberately does not do
//!
//! It answers exactly the methods it is told to answer. An unexpected method is recorded as
//! `Unanswered` and **not replied to**, because a peer that answers anything would let a client that
//! sends the wrong method still complete a negotiation — the failure this suite most needs to be able
//! to see.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// One request the client sent, as it appeared on the wire.
#[derive(Clone, Debug, PartialEq)]
pub struct SeenRequest {
    /// The JSON-RPC id, echoed back by the peer.
    pub id: Value,
    /// The method name.
    pub method: String,
    /// The `params` object, or `null`.
    pub params: Value,
}

/// How the peer answers a method.
#[derive(Clone, Debug)]
enum Reply {
    /// A result object, which is wrapped in a JSON-RPC response envelope.
    Result(Value),
    /// A JSON-RPC error with the given code and message.
    Error { code: i64, message: String },
    /// No reply, and the connection is **closed** immediately.
    ///
    /// The one behaviour a scripted reply cannot express: a peer that simply never answers is
    /// indistinguishable from one that is slow, so a test for "the request went out and no answer came
    /// back" needs the connection to end. That is the case the MCP adapter classifies as
    /// `AmbiguousAfterReaching`, and it is the mapping that decides whether a non-idempotent effect can be
    /// retried — so it needs to be reachable rather than assumed.
    HangUp,
}

/// A scripted MCP server speaking the stdio wire format.
///
/// Scripted rather than intelligent: it answers only what was registered, so a test that forgets to
/// register a method fails loudly instead of receiving an invented answer.
pub struct ScriptedPeer {
    replies: BTreeMap<String, Reply>,
    seen: Vec<SeenRequest>,
}

impl ScriptedPeer {
    /// Creates a peer that answers nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            replies: BTreeMap::new(),
            seen: Vec::new(),
        }
    }

    /// Registers a successful result for a method.
    #[must_use]
    pub fn answering(mut self, method: &str, result: Value) -> Self {
        self.replies
            .insert(method.to_owned(), Reply::Result(result));
        self
    }

    /// Registers a JSON-RPC error for a method.
    #[must_use]
    pub fn failing(mut self, method: &str, code: i64, message: &str) -> Self {
        self.replies.insert(
            method.to_owned(),
            Reply::Error {
                code,
                message: message.to_owned(),
            },
        );
        self
    }

    /// Scripts a method to be answered with **silence and a closed connection**.
    ///
    /// For the case where "the request went out and no answer came back" is the behaviour under test —
    /// the ambiguity an adapter must not confuse with a refusal, because calling it a refusal would invite
    /// a retry that duplicates a non-idempotent effect.
    #[must_use]
    pub fn hanging_up(mut self, method: &str) -> Self {
        self.replies.insert(method.to_owned(), Reply::HangUp);
        self
    }

    /// Returns the requests seen so far, in order.
    ///
    /// Unused by `host.rs`, which asserts on `methods()` and `saw()` instead — but this is a shared
    /// test support module compiled once per test binary, so the lint would fire on the binary that
    /// happens not to call it. Deleting it to satisfy the lint would remove the one accessor that
    /// exposes the full request (params included), which is what an assertion about per-request
    /// metadata needs.
    #[must_use]
    #[allow(
        dead_code,
        reason = "shared test support; used by wire.rs, not by host.rs"
    )]
    pub fn seen(&self) -> &[SeenRequest] {
        &self.seen
    }

    /// Returns the methods seen so far, in order.
    #[must_use]
    pub fn methods(&self) -> Vec<&str> {
        self.seen
            .iter()
            .map(|request| request.method.as_str())
            .collect()
    }

    /// Returns whether a method was ever requested.
    #[must_use]
    pub fn saw(&self, method: &str) -> bool {
        self.seen.iter().any(|request| request.method == method)
    }
}

impl Default for ScriptedPeer {
    fn default() -> Self {
        Self::new()
    }
}

/// The server's half of a duplex pair, driven as a scripted peer.
///
/// Spawned as a task rather than run inline, because the client blocks on the response: a peer that
/// only replies after the client returns would deadlock, and a deadlock in this suite would look like
/// a hang rather than a failure.
pub struct PeerHandle {
    task: tokio::task::JoinHandle<ScriptedPeer>,
}

impl PeerHandle {
    /// Spawns a scripted peer on the server half of a duplex pair.
    #[must_use]
    pub fn spawn(mut server_side: DuplexStream, peer: ScriptedPeer) -> Self {
        let task = tokio::spawn(async move {
            let (read_half, mut write_half) = tokio::io::split(&mut server_side);
            let mut reader = BufReader::new(read_half);
            let mut peer = peer;
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                    // A peer that silently skipped a malformed frame would hide a framing defect in
                    // the client, which is the one thing this suite most needs to detect.
                    break;
                };
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let method = message
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                peer.seen.push(SeenRequest {
                    id: id.clone(),
                    method: method.clone(),
                    params: message.get("params").cloned().unwrap_or(Value::Null),
                });

                // A notification has no id and takes no reply.
                if id.is_null() {
                    continue;
                }
                let Some(reply) = peer.replies.get(&method) else {
                    // Deliberately unanswered. See the module doc: replying to anything would let a
                    // client that called the wrong method still appear to negotiate.
                    continue;
                };
                let envelope = match reply {
                    Reply::Result(result) => {
                        json!({ "jsonrpc": "2.0", "id": id, "result": result })
                    }
                    Reply::Error { code, message } => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": code, "message": message }
                    }),
                    // The connection ends with no reply. `break` rather than `continue`, so the pipe is
                    // closed and the client observes a dead peer rather than a silent one.
                    Reply::HangUp => break,
                };
                let mut bytes = serde_json::to_vec(&envelope).unwrap_or_default();
                bytes.push(b'\n');
                if write_half.write_all(&bytes).await.is_err() {
                    break;
                }
                let _ = write_half.flush().await;
            }
            peer
        });
        Self { task }
    }

    /// Waits for the peer to finish and returns everything it saw.
    pub async fn finish(self) -> ScriptedPeer {
        self.task
            .await
            .unwrap_or_else(|error| panic!("peer task failed: {error}"))
    }
}

/// The `server/discover` result a modern server sends.
///
/// Every field is the wire name the SDK deserializes, so a field rename in the SDK fails these tests
/// rather than passing silently.
#[must_use]
pub fn discover_result(server_name: &str, server_title: Option<&str>, tools: bool) -> Value {
    let mut implementation = json!({ "name": server_name, "version": "1.2.3" });
    if let Some(title) = server_title {
        implementation["title"] = Value::String(title.to_owned());
    }
    let mut capabilities = json!({});
    if tools {
        capabilities["tools"] = json!({ "listChanged": true });
    }
    json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": capabilities,
        "ttlMs": 0,
        "cacheScope": "private",
        // The identity is carried in result metadata, not as a top-level field: the SDK reads
        // `io.modelcontextprotocol/serverInfo` out of `_meta` (model.rs, `server_info_from_meta`).
        // A top-level `serverInfo` is accepted by nothing and would leave the identity absent.
        "_meta": { "io.modelcontextprotocol/serverInfo": implementation }
    })
}

/// A `tools/list` result carrying the given tools.
#[must_use]
pub fn tools_result(tools: &[Value]) -> Value {
    json!({
        "resultType": "complete",
        "tools": tools,
        "ttlMs": 0,
        "cacheScope": "private"
    })
}

/// One tool entry, in the shape a modern server sends.
#[must_use]
pub fn tool(name: &str, title: Option<&str>, description: Option<&str>) -> Value {
    let mut entry = json!({
        "name": name,
        "inputSchema": { "type": "object", "properties": {} }
    });
    if let Some(title) = title {
        entry["title"] = Value::String(title.to_owned());
    }
    if let Some(description) = description {
        entry["description"] = Value::String(description.to_owned());
    }
    entry
}

/// A tool carrying annotations a hostile or careless server might send.
///
/// Used to prove the translation cannot consult them: the hints claim the tool is read-only and
/// idempotent, which is exactly what a server would assert to get an effect auto-approved.
#[must_use]
pub fn announced_safe_tool(name: &str) -> Value {
    let mut entry = tool(name, None, None);
    entry["annotations"] = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    });
    entry
}
