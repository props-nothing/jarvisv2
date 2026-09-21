---
integration: local-daemon-protocol
status: implemented
last_verified: 2026-09-21
owners: []
selected_spec_version: 1
selected_sdk: tokio 1.53.1
---

# Local Daemon Protocol And Native IPC

## Scope

The same-machine transport and versioned control protocol between local clients (`jarvis-cli`, later `jarvis-desktop`) and the `jarvisd` daemon: framing, handshake, version negotiation, authentication, endpoint naming, and the `status` / `health` commands. Remote HTTP/WebSocket surfaces, streaming events, and the authenticated loopback HTTP fallback are out of scope for this record.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | not found for the Rust standard library, Tokio, or `getrandom` | 2026-09-21 | discovery |
| Tokio `net` module | https://docs.rs/tokio/1.53.1/tokio/net/index.html | 2026-09-21 | normative transport contract |
| Tokio `named_pipe::ServerOptions` | https://docs.rs/tokio/1.53.1/tokio/net/windows/named_pipe/struct.ServerOptions.html | 2026-09-21 | pipe creation flags |
| Tokio `io::duplex` | https://docs.rs/tokio/1.53.1/tokio/io/fn.duplex.html | 2026-09-21 | in-process test streams |
| Rust `PermissionsExt` | https://doc.rust-lang.org/std/os/unix/fs/trait.PermissionsExt.html | 2026-09-21 | socket and credential modes |
| Microsoft named pipes | https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes | 2026-09-21 | pipe security and semantics |
| Microsoft `CreateNamedPipe` | https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea | 2026-09-21 | `FILE_FLAG_FIRST_PIPE_INSTANCE`, `PIPE_REJECT_REMOTE_CLIENTS` |
| `getrandom` 0.4.3 | https://docs.rs/getrandom/0.4.3/getrandom/ | 2026-09-21 | credential entropy |
| changelog/release notes | not applicable; pinned versions verified through `Cargo.lock` | 2026-09-21 | compatibility |

Where no `llms.txt` exists, the normative contract is the language/OS specification, and implementation details were read from the pinned crate source in the local cargo registry.

## Verified Contract

### Operations And Transport

- Unix: `UnixListener::bind(path)` in the private runtime directory; the socket file is set to mode `0600` and removed when the listener is dropped.
- Windows: byte-mode named pipe `\\.\pipe\jarvis-<profile>-<root-hash>` created with `ServerOptions`.
- Framing: 4-byte big-endian payload length, then UTF-8 JSON. `MAX_FRAME_BYTES = 64 * 1024` including the prefix; `MIN_PAYLOAD_BYTES = 2`.
- Message order: one client handshake, one server handshake, then request/response pairs. `HANDSHAKE_TIMEOUT = 10s`; `MAX_REQUESTS_PER_CONNECTION = 256`.
- Commands in v1: `status` and `health`.

### Authentication And Authorization

OS-protected IPC plus a profile-bound credential (`docs/architecture/security.md`). The credential is 32 bytes from the operating-system random source, stored beside the profile configuration, and hardened to current-user-only access (Unix `0600`; the Windows configuration directory is restricted to the current account by `AppPaths::prepare`). Comparison uses a length-independent, content-constant-time routine, and `Debug`/`Display` never reveal the value. Locality is not authorization; every connection must present the credential in the handshake.

The credential is **issued only by the daemon**. A client reads the file and refuses to proceed when it is missing; a client that generated a value on absence would let any local process choose the secret the daemon trusts. The client reports the missing credential and exits `3` instead.

### Limits And Failure Semantics

- An oversized declared length is rejected before allocation; a close inside a frame is `Truncated`; a malformed payload is `Malformed`.
- Negotiation selects the highest common version and fails closed on an inverted window, a too-old client, or a too-new client with distinct errors.
- The daemon replies with a `Rejected` handshake or a `WireError` envelope; both carry a stable `ErrorCode`, a bounded `SafeMessage`, retryability, and a correlation ID. A rejection is never a bare success signal.

### Measured Windows Findings

1. `reject_remote_clients` already defaults to `true`; it is set explicitly for intent.
2. `first_pipe_instance(true)` maps to `FILE_FLAG_FIRST_PIPE_INSTANCE`, which **fails when any instance of the name already exists**. It must apply only to the daemon's initial instance; applying it to the next pending instance produced a broken pipe because the daemon's own listener already existed.
3. The next instance must be created **after** the current instance connects. Two simultaneous listening instances of one name let the client attach to the wrong one, and its first write failed with "the pipe is being closed" (Win32 error 232).
4. `poll_flush` on a named pipe is a no-op returning `Ready(Ok(()))`, so a write failure surfaces on `write_all`.

### Data And Compliance

No payload leaves the machine. The credential is local-only and is never logged, serialized outside the handshake, or included in errors.

### Versions And Deprecations

`tokio = "=1.53.1"` (features `io-util`, `net`, `time`), `getrandom = "0.4.3"`. Protocol version 1 has a supported window of `[1, 1]`; negotiation is written so older clients keep working when the window widens.

## JARVIS Mapping

- Endpoint locator: `jarvis_core::LocalEndpoint`.
- Secret: `jarvis_core::ClientCredential` (value) with `jarvis_storage::CredentialStore` (persistence).
- Wire DTOs: `jarvis_protocol::{ClientHandshake, ServerHandshake, HandshakeOutcome, DaemonHandshake, Request, Response, Outcome, Reply, StatusReply, HealthReply, WireError}`.
- Errors reuse `jarvis_core::{ErrorCode, SafeMessage, CorrelationId}`; `WireError::retryable` derives from `ErrorCode::is_retryable`.
- No provider SDK type crosses the boundary.

## Decisions

- Native IPC only; the authenticated loopback fallback is not built because no platform/library constraint requires it yet.
- A profile-bound credential rather than relying on socket/pipe permissions alone.
- Length-prefixed JSON rather than a self-describing framing, so the size bound is enforceable before allocation.
- Endpoint naming derived from the profile name and the runtime directory (`\\.\pipe\jarvis-<profile>-<root-hash>`). Windows pipe names occupy a single machine-global namespace, so the runtime directory is folded in through a stable FNV-1a digest; a profile-name-only name would make a portable `default` collide with a natively installed `default`. Unix needs no equivalent because the socket already lives inside the runtime directory.
- The `ErrorCode::Unsupported` variant was added for "this build or platform cannot do that".

## Rejected Alternatives

- PID-file-only liveness: the kernel file lock remains the singleton authority.
- Trusting `SO_PEERCRED` as authentication: the standard library does not expose it portably, and a credential in the handshake covers every platform uniformly.
- Unbounded framing or unbounded requests per connection: both are explicitly capped.
- Creating the next pipe instance before the current one connects: measured to fail (finding 3 above).
- Deriving the Windows pipe name from the profile name alone: it collides across installations that share a profile name, which was observed by running portable and native daemons concurrently.

## Evidence Of Implementation

- `crates/jarvis-core/src/endpoint.rs`, `transport.rs`, `credential.rs`
- `crates/jarvis-protocol/src/frame.rs`, `version.rs`, `wire.rs`, `session.rs`
- `crates/jarvis-storage/src/credential.rs`
- `crates/jarvis-protocol/tests/local_transport.rs` (real socket/pipe handshake and refusal)
- `apps/jarvisd/src/control.rs`, `apps/jarvis-cli/src/main.rs`, `output.rs`
- Manual verification: `jarvis status` and `jarvis health --json` succeed against a running daemon; a tampered credential yields `authentication` with exit code 4.

## Unresolved Ambiguities

- Connection concurrency limits and maximum in-flight connections are undefined; the daemon accepts sequentially and spawns a task per connection.
- No keepalive frame; idle connections close only when a peer closes them.
- macOS `AF_UNIX` path length (~104 bytes) is unchecked, so a long runtime-directory path could fail to bind there. Native CI must confirm.
