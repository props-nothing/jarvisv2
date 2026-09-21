---
integration: portable-and-service-lifecycle
status: implemented
last_verified: 2026-09-21
owners: []
selected_spec_version: 1
selected_sdk: none (standard library only)
---

# Portable Mode And Per-User Service Lifecycle

## Scope

`FR-INSTALL-003` and `P1-011`: portable foreground mode with an explicit data
directory, plus service lifecycle **abstractions**. Service *installation* is out of
scope by the task itself ("without installing services yet"), as are keychain,
installer, updater, and container behavior.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | not found; these are OS and language specifications | 2026-09-21 | discovery |
| systemd user units | https://www.freedesktop.org/software/systemd/man/latest/systemd.service.html | 2026-09-21 | unit file shape |
| launchd agents | https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html | 2026-09-21 | plist shape and `LaunchAgents` location |
| Windows named pipes (namespace) | https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipes | 2026-09-21 | why pipe names are global |
| Rust `Path` and `File::try_lock` | https://doc.rust-lang.org/std/path/struct.Path.html | 2026-09-21 | absolute-path validation and lock probing |

Repository-internal normative sources: `docs/operations/install-and-release.md`
(Installation Modes, Application Paths, Per-User Service, reconcile steps),
`docs/architecture/overview.md` (Portable), `docs/research/integrations/os-application-directories.md`.

## Verified Contract

### Portable Mode

`docs/operations/install-and-release.md`: "Extract and run with an explicit
profile/data directory. No PATH, service, registry, LaunchAgent, or systemd
changes." `os-application-directories.md` adds: "Use only an explicit
caller-provided root for later portable mode; do not silently use the current
directory."

Implemented as `jarvisd --root <absolute-directory>` and `jarvis --root ...`. The
root is validated as absolute and the layout is created *inside* it
(`config`, `data`, `cache`, `state`, `runtime`, `logs`). A relative root is a usage
error (exit 2) rather than a resolved path.

### Measured portable acceptance (Windows, live)

Against a fresh empty directory:

1. `jarvisd --root <root>` logged `mode="portable"` and became ready.
2. `jarvis --status --root <root>` returned the same daemon, exit 0.
3. Every file created was inside the root: `config/client.credential`,
   `data/jarvis.sqlite3` (+ WAL/SHM), `logs/jarvisd.jsonl`, `runtime/jarvisd.lock`.
   Nothing appeared under `%APPDATA%\JARVIS` or `%LOCALAPPDATA%\JARVIS`.
4. A **native** daemon was started at the same time. Both clients succeeded and
   reported **different** `daemon_id` values, proving the two profiles did not
   contend for one transport.

### Service Lifecycle

`install-and-release.md` defines installation as a seven-step reconcile and states
"never rewrite an externally managed service without explicit operator action". This
slice implements steps 1 (resolve paths), 2 (read and classify), and 6 (verify),
and deliberately implements no part of steps 3–5 or 7: staging, loading, starting,
and rollback are the operator action that `P1-011` excludes.

Classification (`ServiceDrift`):

| State | Meaning | Replaceable without operator action |
| --- | --- | --- |
| `absent` | no definition exists | yes |
| `current` | an owned definition matches exactly | yes (no-op) |
| `drifted` | an owned definition differs | yes |
| `foreign` | a definition without the ownership marker | **no** |
| `unreadable` | the definition could not be read | **no** |

Ownership is decided by the `jarvis-managed-service` marker inside the definition,
not by its path: an operator may legitimately have placed an unrelated definition at
the same location.

## Decisions

- **The endpoint is derived from the runtime directory, not just the profile name.**
  Windows pipe names occupy a single global namespace, so a portable profile named
  `default` would otherwise collide with a natively installed `default`. The root is
  folded into the pipe name through a stable FNV-1a digest, so the endpoint stays
  derived (never configured in a file) and identical for every client sharing the
  root. Verified by running both profiles at once.
- **Portable mode refuses to plan a service.** Portable mode is *defined* by leaving
  no service behind, so `ServicePlan::build` returns `PortableMode` rather than
  quietly installing one.
- **The service launches the daemon, not the client.** `current_exe()` in `jarvis` is
  the client; using it directly planned a launcher that would never serve anything.
  The daemon is resolved as the sibling binary, and absence is an error rather than a
  fallback. This defect was observed live before being fixed.
- **Drift detection normalizes line endings**, so a CRLF checkout is not reported as
  drift.
- **No service definition is written in this slice.** A planner that also wrote files
  could not be tested without mutating a machine.

## Rejected Alternatives

- Resolving a relative `--root` against the working directory: explicitly forbidden,
  because state would scatter wherever the process started.
- Using a random or build-time constant for the pipe name suffix: a random value
  breaks the clients it is meant to connect, and a constant reintroduces the
  collision.
- Adopting any definition found at the canonical path: an unowned service must
  require explicit operator action.
- Implementing `service install` now: out of scope, and it would need the staging,
  activation, readiness-wait, and rollback machinery to be meaningful.

## Evidence Of Implementation

- `crates/jarvis-storage/src/paths.rs` — `portable_layout`, `PathMode`, validation
- `crates/jarvis-storage/tests/portable_mode.rs` — 6 acceptance tests
- `crates/jarvis-core/src/endpoint.rs` — `named_pipe_in_root`, `scoped`, collision tests
- `crates/jarvis-diagnostics/src/service.rs` — `ServicePlan`, `detect_drift`,
  `daemon_binary_beside`, `drift_finding`
- `apps/jarvisd/src/main.rs` — `--root` parsing and mode reporting
- `apps/jarvis-cli/src/main.rs` — `--root`, `jarvis service`

## Unresolved Ambiguities

- The service definitions are rendered but never written, loaded, or started.
  Readiness waiting, atomic staging, and rollback remain for the installation slice.
- Definition paths are derived from `USERPROFILE`/`HOME`; the Windows location
  (`.jarvis/service`) is a placeholder until the installer slice fixes the identity.
- No check verifies that a running service's binary version matches the installed
  one; that needs live process inspection.
- macOS `AF_UNIX` socket path length (~104 bytes) is still unchecked for a deep
  portable root. Native CI must confirm.
