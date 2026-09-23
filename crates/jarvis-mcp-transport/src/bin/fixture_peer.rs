//! A minimal MCP server on stdio, used as a **real child process** in this crate's tests.
//!
//! # What this exists for
//!
//! `P3-008e` recorded an honest limit in plain words: `connect_stdio` and `connect_http` were
//! **entirely unexercised**, because every wire test drove `connect_over` over an in-process duplex
//! pair. A duplex pair proves the framing and the negotiation; it cannot prove that a *spawned child
//! process* is reachable, that its stdio is wired the way the transport assumes, or that a real OS
//! spawn error surfaces as a refusal rather than a hang. Those are exactly the failures a user would
//! hit first, and they are the ones no in-process fixture can show.
//!
//! So this is a separate executable that speaks the same newline-delimited JSON-RPC framing the
//! transport expects — written by hand rather than with the SDK, for the reason the scripted peer in
//! `tests/support` is: a client and a server built from one library share their assumptions, so a
//! disagreement with the *specification* would pass. This program writes bytes to stdout and reads
//! bytes from stdin, and if the framing were wrong every stdio test would fail rather than pass.
//!
//! # Configuration, so one fixture can be several peers
//!
//! Arguments choose the reported identity and the offered tools, so the same binary can stand in for
//! a server that reports one name and a server that reports another. Defaults keep the common case
//! (a well-formed modern server with two tools) free of arguments.
//!
//! # It is not part of the product
//!
//! A `[[bin]]` so the tests can spawn it through the same OS path a user's configured server would
//! take. It is declared as `required-features = ["fixture-peer"]`, which is **not** in the default
//! feature set, so `cargo build --workspace` does not build it and it cannot be mistaken for a
//! shipped binary. The tests that need it enable the feature.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// Reads arguments of the form `--name <server name>` and `--tool <tool name>`, repeatable.
fn parse_arguments() -> (String, Vec<String>) {
    let mut name = "fixture-vendor".to_owned();
    let mut tools = vec!["search".to_owned(), "fetch".to_owned()];
    let mut explicit_tools = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--name" => {
                if let Some(value) = arguments.next() {
                    name = value;
                }
            }
            "--tool" => {
                if let Some(value) = arguments.next() {
                    explicit_tools.push(value);
                }
            }
            // An unrecognized argument is ignored rather than refused: this is a test fixture, and a
            // fixture that exited on an unexpected flag would make a test failure look like a crash
            // in the transport instead of a mistake in the test.
            _ => {}
        }
    }
    if !explicit_tools.is_empty() {
        tools = explicit_tools;
    }
    (name, tools)
}

/// The `server/discover` result, in the shape a modern server sends.
///
/// The identity is carried in `_meta` under the specification's key rather than as a top-level field,
/// because that is where the SDK reads it from — a top-level `serverInfo` is accepted by nothing and
/// would leave the identity absent, which would silently weaken every identity assertion.
fn discover_result(name: &str) -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": ["2026-07-28"],
        "capabilities": { "tools": { "listChanged": true } },
        "ttlMs": 0,
        "cacheScope": "private",
        "_meta": {
            "io.modelcontextprotocol/serverInfo": { "name": name, "version": "1.0.0" }
        }
    })
}

/// The `tools/list` result for the given tool names.
fn tools_result(tools: &[String]) -> Value {
    let entries: Vec<Value> = tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool,
                "inputSchema": {
                    "type": "object",
                    "properties": { "q": { "type": "string" } }
                }
            })
        })
        .collect();
    json!({
        "resultType": "complete",
        "tools": entries,
        "ttlMs": 0,
        "cacheScope": "private"
    })
}

/// The `tools/call` result for a call, echoing the tool name back in the text.
fn call_result(tool: &str) -> Value {
    json!({
        "resultType": "complete",
        "content": [{ "type": "text", "text": format!("{tool} ran") }],
        "isError": false
    })
}

fn main() {
    let (name, tools) = parse_arguments();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        // `read_line` returning 0 means the parent closed the pipe, which is the shutdown signal.
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A frame this program cannot parse ends the loop rather than being skipped: skipping would
        // hide a framing defect in the client, which is the one thing these tests must be able to
        // detect.
        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
            return;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        if id.is_null() {
            // A notification takes no reply.
            continue;
        }
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let response = match method {
            "server/discover" => {
                json!({ "jsonrpc": "2.0", "id": id, "result": discover_result(&name) })
            }
            "tools/list" => json!({ "jsonrpc": "2.0", "id": id, "result": tools_result(&tools) }),
            "tools/call" => {
                let called = message
                    .get("params")
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                json!({ "jsonrpc": "2.0", "id": id, "result": call_result(called) })
            }
            // An unscripted method is answered with a protocol error rather than silence, so a client
            // that calls something this fixture does not implement gets a diagnosable refusal instead
            // of a timeout that looks like a transport fault.
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }),
        };

        let Ok(mut bytes) = serde_json::to_vec(&response) else {
            return;
        };
        bytes.push(b'\n');
        if stdout.write_all(&bytes).is_err() || stdout.flush().is_err() {
            return;
        }
    }
}
