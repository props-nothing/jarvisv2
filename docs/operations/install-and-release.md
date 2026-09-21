# Installation And Release Plan

This is the required product behavior. No installer/release exists yet.

## Artifact Matrix

Initial targets:

| OS | Architecture | CLI/daemon | Desktop |
| --- | --- | --- | --- |
| Windows | x86_64 | required | required when Phase 9 ships |
| Windows | ARM64 | evaluate/native CI required before claim | evaluate |
| macOS | Apple Silicon | required | required |
| macOS | Intel | best effort only if CI/signing remains viable | best effort |
| Linux | x86_64 | required | required |
| Linux | ARM64 | required for server/edge | evaluate desktop |

Build each release on native or officially supported CI. Do not claim a target from an untested cross-compile alone.

Artifacts include version, target triple, archive/package type, checksum, signature, SBOM, and provenance. `jarvis` and `jarvisd` versions are released together in v1.

## Installation Modes

### Bootstrap script

```text
curl -fsSL https://<official-domain>/install.sh | sh
iwr -useb https://<official-domain>/install.ps1 | iex
```

These commands are documentation targets, not active URLs. Scripts are small bootstrap verifiers that download a signed release; they do not install a language runtime or execute unverified nested scripts.

### Native packages

- Windows installer/package with signed binaries
- macOS signed and notarized app/package
- Linux tarball plus appropriate AppImage/deb/rpm where maintained
- Tauri desktop bundles include or coordinate the matching daemon/CLI

### Portable

Extract and run with an explicit profile/data directory. No PATH, service, registry, LaunchAgent, or systemd changes.

Implemented as `jarvisd --root <absolute-directory>` and `jarvis <command> --root <absolute-directory>`. The root must be absolute: a relative value is a usage error rather than a path resolved against the current directory. Every managed directory (`config`, `data`, `cache`, `state`, `runtime`, `logs`) is created inside the root, and the local endpoint is derived from the root as well as the profile name, so a portable profile and a natively installed profile with the same name can run concurrently without contending for one socket or pipe. Portable mode installs no service.

### Container/server

Optional image for headless server mode. It is not the recommended personal-desktop install and does not grant host desktop/audio control automatically.

## Application Paths

Use platform path APIs. Exact application identifiers are finalized before implementation.

| OS | Config | Data/state | Cache/logs | Local transport |
| --- | --- | --- | --- | --- |
| Windows | `%APPDATA%\JARVIS` | `%LOCALAPPDATA%\JARVIS` | local app data subdirs | named pipe |
| macOS | `~/Library/Application Support/JARVIS` | same owned tree | `~/Library/Caches/JARVIS`, `~/Library/Logs/JARVIS` | Unix socket in secure runtime/data dir |
| Linux | `$XDG_CONFIG_HOME/jarvis` | `$XDG_DATA_HOME/jarvis`, `$XDG_STATE_HOME/jarvis` | `$XDG_CACHE_HOME/jarvis` and state logs | `$XDG_RUNTIME_DIR` Unix socket |

Directories containing state or IPC endpoints use user-only permissions/ACLs. `doctor` detects unsafe ownership/permissions and path mismatch between CLI and installed service.

## Per-User Service

- Linux: systemd user service.
- macOS: launchd LaunchAgent.
- Windows: per-user Scheduled Task/logon launcher by default to avoid forced elevation; an optional Windows Service mode may be designed for server/shared-machine deployments.

Service installation is a reconcile operation:

1. Resolve the exact verified binary and profile paths.
2. Read existing owned definition and determine drift/ownership.
3. Stage new definition atomically.
4. Load/start through the platform manager.
5. Wait for process plus RPC readiness.
6. Verify service binary version, config/state path, and daemon protocol.
7. Roll back staged definition when activation fails.

Never rewrite an externally managed service without explicit operator action.

### Implemented Planning Surface (P1-011)

`jarvis service` implements steps 1, 2, and 6 and is strictly read-only: it resolves
the daemon binary (beside the client, never the client itself), renders the
definition the platform manager would use, and classifies the existing definition.

| Drift | Meaning | Replaceable without operator action |
| --- | --- | --- |
| `absent` | no definition exists | yes |
| `current` | an owned definition matches exactly | yes (no-op) |
| `drifted` | an owned definition differs | yes |
| `foreign` | a definition without the `jarvis-managed-service` marker | **no** |
| `unreadable` | the definition could not be read | **no** |

Ownership is decided by the marker inside the definition, not by its path. Staging,
activation, readiness waiting, and rollback are not implemented; that is the
installation slice, and this task explicitly excludes installing services.

## Installer Flow

1. Detect OS, architecture, shell, install scope, existing installations, and profile.
2. Resolve the requested stable/beta/pinned release from a signed manifest.
3. Download to a temporary owned directory with size/time bounds.
4. Verify manifest trust, artifact checksum, and signature before extraction/execution.
5. Stage binaries; keep an existing working version.
6. Add/update PATH using platform-safe methods.
7. Run version/config/schema preflight.
8. Launch onboarding or print a non-interactive next command.
9. Optionally reconcile the per-user service.
10. Run `jarvis doctor` and health probe.
11. Write an install receipt containing no secrets.

The installer is idempotent and accepts non-interactive flags. It never hides an elevation prompt; elevated modes require the user to run an explicit platform command.

## Onboarding

First launch:

1. Choose local/portable/server mode.
2. Create local user and workspace.
3. Select a model provider or local endpoint and test it.
4. Initialize storage and show data location.
5. Optionally install background service.
6. Optionally connect a first account.
7. Optionally configure voice after text works.
8. Show privacy/data-flow summary and tool approval defaults.
9. Run doctor and one deterministic local self-test.

Onboarding is resumable and never stores a connector account before successful identity/connectivity verification.

## Doctor

Checks include:

- binary/client/daemon/protocol version alignment
- config parse/schema/future-version errors
- config/data/cache/log path agreement and permissions
- singleton lock, local socket/pipe, ports, readiness
- database integrity/migration/backup status
- secret-provider access without displaying values
- model/runtime/connector health and missing scopes
- MCP server definitions, executable identity, protocol compatibility
- service definition drift and duplicate installations
- update state, available rollback, signing trust
- disk space, clock/timezone, TLS/proxy configuration
- redaction self-test and unsafe remote exposure

Checks emit stable IDs, severity, facts, remediation, whether repair is safe, and post-repair verification. `--json` is suitable for support automation.

## Update

1. Acquire an update/maintenance lock and record a durable update run.
2. Fetch and verify a signed release manifest/artifact.
3. Check target schema/config compatibility and free space.
4. Create/verify required backup.
5. Stage the new binary separately.
6. Ask the daemon to drain/handoff; stop only after bounded wait.
7. Atomically switch launcher/current pointer.
8. Start and probe the new daemon plus client protocol.
9. Run post-update doctor checks and migrations.
10. Finalize, retaining rollback according to policy.

On failure, restore service definition/binary and compatible state. A database migration that prevents binary rollback must be explicitly identified before activation and provide a supported restore path.

## Signing And Supply Chain

- protect release signing in a dedicated CI/environment with least privilege
- pin third-party actions by immutable revision
- publish checksums, signatures, SBOM, build provenance, source revision, and dependency lock
- scan dependencies, licenses, secrets, and artifacts
- verify installers from clean machines using only published trust roots
- document key rotation and compromise response before public release

## Release CI

Stages:

1. format/lint/unit/repository tests
2. contract/integration/security checks
3. native target builds
4. package/install smoke in clean environments
5. database upgrade/backup/restore and update/rollback
6. selected opt-in live-provider and end-to-end lanes
7. artifact signing/provenance/SBOM
8. publish candidate channel
9. post-publish install verification
10. promote immutable candidate to stable

Never rebuild a different artifact during promotion.

## Uninstall

Uninstall separates:

- stop/remove service
- remove binaries/PATH/desktop app
- retain or delete config
- retain or delete canonical data/artifacts/logs/backups
- delete keychain/secret entries
- revoke connector/provider credentials where supported

Defaults preserve user data and explain its location. Full deletion requires explicit confirmation or non-interactive flags and emits a manifest/receipt without sensitive content.

## Operations Before Stable

- backup/restore runbook and rehearsed recovery objectives
- remote exposure and trusted-proxy runbook
- incident response and signing-key rotation
- support bundle schema and privacy review
- release channel/rollback support policy
- supported OS/architecture and database compatibility table
- provider-cost and live-test budgets