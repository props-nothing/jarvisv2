---
integration: rust-daemon-lifecycle
status: implemented
last_verified: 2026-09-20
owners: [daemon]
selected_spec_version: Rust 1.98.1 standard library
selected_sdk: Tokio 1.53.1
---

# Rust Daemon Lifecycle

## Scope

This record covers the local `jarvisd` composition root, per-profile singleton
locking, process signals, graceful shutdown, liveness/readiness state, and build
metadata for `P1-007`. Local client IPC, service installation, remote control,
and restart supervision are out of scope. The portable `--root <absolute-directory>`
flag was added by `P1-011` and selects the same lifecycle over an explicit root; it
is recorded in [portable-and-service-lifecycle.md](portable-and-service-lifecycle.md).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | Rust and Tokio have no usable official `llms.txt`; official rustdoc and Cargo Book used | 2026-09-20 | discovery attempt |
| Rust `File` rustdoc | https://doc.rust-lang.org/1.98.0/std/fs/struct.File.html | 2026-09-20 | lock, try-lock, unlock, and close behavior |
| Rust `TryLockError` rustdoc | https://doc.rust-lang.org/1.98.0/std/fs/enum.TryLockError.html | 2026-09-20 | contention versus I/O failure |
| Tokio portable Ctrl-C | https://docs.rs/tokio/1.53.1/tokio/signal/fn.ctrl_c.html | 2026-09-20 | portable shutdown notification |
| Tokio Unix signals | https://docs.rs/tokio/1.53.1/tokio/signal/unix/ | 2026-09-20 | SIGTERM handling |
| Tokio Windows signals | https://docs.rs/tokio/1.53.1/tokio/signal/windows/ | 2026-09-20 | console close, break, logoff, and shutdown handling |
| Cargo environment variables | https://doc.rust-lang.org/cargo/reference/environment-variables.html | 2026-09-20 | compile-time package metadata |
| Tokio package and feature metadata | https://crates.io/crates/tokio/1.53.1 | 2026-09-20 | selected version, license, MSRV, and narrow features |

## Verified Contract

### Operations And Transport

Rust's stable `File::try_lock` acquires an exclusive operating-system lock and
returns `TryLockError::WouldBlock` when another handle holds a conflicting lock.
The implementation currently maps to `flock(LOCK_EX | LOCK_NB)` on Unix and
`LockFileEx` with immediate exclusive acquisition on Windows. The file must be
opened for writing; the lock is released by `unlock` or when the last duplicated
handle closes.

Tokio's `signal::ctrl_c` is portable. Unix additionally supports SIGTERM through
`signal(SignalKind::terminate())`; Windows exposes Ctrl-C, Ctrl-Break,
Ctrl-Close, Ctrl-Logoff, and Ctrl-Shutdown streams. Polling Tokio signal streams
replaces the process's default handler, so JARVIS must always settle shutdown
after receiving one rather than dropping the listener and expecting default
termination to return.

Cargo provides `CARGO_PKG_VERSION` at compile time. Target OS and architecture
are compile-time Rust constants. No build script or source-control command is
required for the Phase 1 build identity.

### Authentication And Authorization

No network authentication is involved. The singleton file lives in the private
per-user runtime directory. Possession of the file is not authority; the held
kernel lock is the only singleton evidence.

### Limits And Failure Semantics

Lock acquisition is non-blocking. Contention maps to a stable "already running"
startup failure; all other lock errors remain distinct I/O failures. A stale
lock file is harmless after a crash because locks are tied to open handles, not
file contents.

The lock file is retained across clean exits. Deleting a locked Unix file would
allow another process to create and lock a new inode while the first process
still holds the old inode, violating singleton behavior. Diagnostic lock-file
content is bounded and contains only process/build metadata.

Signal registration can fail and is a startup/runtime error. Shutdown is one
way: readiness becomes false, owned resources close, and the singleton guard is
released last. A second signal may be handled as forced termination in a later
service-supervision task; `P1-007` performs one bounded graceful path.

### Data And Compliance

No data leaves the machine. Lock metadata contains process ID and non-sensitive
build identity only. Tokio is MIT licensed; Rust standard-library behavior adds
no package dependency.

### Versions And Deprecations

- File locking is stable since Rust 1.89.0; the workspace pins Rust 1.98.1.
- Tokio 1.53.1 is stable, MIT licensed, and requires Rust 1.71.
- The daemon enables only the Tokio runtime, macros, signal, and synchronization
  features needed by the lifecycle and later local listener.

## JARVIS Mapping

The lock guard belongs to the daemon composition root and never crosses into
domain policy. Liveness means the bootstrap/event loop is running. Readiness is
false while booting or stopping and true only after native paths, singleton
lock, configuration, and SQLite are ready. Build information includes daemon
version, target OS/architecture, configuration version, and database schema
version without hostnames, usernames, paths, or environment values.

## Decisions

- Use the Rust standard library's exclusive file lock; add no locking crate.
- Keep one `jarvisd.lock` file in the private runtime directory permanently and
  hold its open handle for the daemon lifetime.
- Distinguish lock contention from malformed/symlink/permission/I/O failures.
- Handle Ctrl-C everywhere, SIGTERM on Unix, and close/shutdown console events
  on Windows through Tokio 1.53.1.
- Close SQLite before dropping the singleton guard.

## Rejected Alternatives

- PID-file existence is rejected because crashes leave stale files and PID reuse
  makes ownership ambiguous.
- Deleting the lock file on shutdown is rejected because unlinking a held Unix
  inode can split the lock domain.
- A TCP-port singleton is rejected because it couples lifecycle to a transport
  not selected until `P1-008` and can conflict with unrelated applications.
- A third-party locking crate is rejected because Rust 1.98.1 supplies the
  required cross-platform primitive directly.

## Verification Plan

- Acquire a guard, prove a second handle reports contention, drop the first, and
  prove acquisition succeeds again without deleting the file.
- Reject a symbolic-link lock endpoint and a non-file endpoint.
- Prove lock metadata is bounded, contains the current PID/version, and leaves
  no secret-bearing environment values.
- Drive the daemon lifecycle with an injected shutdown future: booting is not
  ready, initialized is ready, stopping is live but not ready, and resources
  close before guard release.
- Launch the real binary in a temporary portable profile (`jarvisd --root <dir>`),
  observe readiness, start a second instance, send a native shutdown event, and
  verify clean exit.

The cheapest test that disproves the central assumption acquires two independent
handles to the same lock file and requires the second to return the dedicated
contention result until the first guard is dropped.

## Unresolved Questions

Native console-event delivery and termination timing vary by operating system;
they require the Windows, macOS, and Linux native CI scenarios in `P1-012`.
This does not block the portable lifecycle implementation.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | Rust 1.98.1; Tokio 1.53.1 | Windows lock contention/reacquisition, health transitions, durable lifecycle records, signal helpers, build output, strict Clippy, and workspace tests pass | GitHub Copilot |