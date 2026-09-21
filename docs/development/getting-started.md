# Development Getting Started

## Current State

The repository contains the architecture baseline, the Rust workspace from `P1-001`, and a Python reference prototype.

Phase 1 (`P1-001` through `P1-012`) has delivered a runnable local foundation:

- `jarvisd` daemon lifecycle, singleton lock, graceful shutdown, readiness, and liveness
- OS-native config/data/cache/runtime/log paths with restrictive permissions
- versioned configuration (schema v1) with migrations, unknown-key errors, and atomic writes
- SQLite schema with migrations, schema-version checks, and a verified pre-migration backup
- local control protocol v1 over a Unix domain socket or a Windows named pipe, authenticated with a profile-bound credential
- structured logs: one JSON object per line, bounded and secret-redacted, plus human console output from the same event
- `jarvis status`, `jarvis health`, `jarvis logs`, `jarvis doctor`, and `jarvis service` with human and `--json` output
- offline `doctor` diagnosis with stable finding codes, safe evidence, specific remediation, and verified repair
- portable foreground mode where one explicit root holds every managed file
- per-user service planning and drift detection (no service is installed yet)
- an automated process-level acceptance gate in `tests/e2e`, run on Windows, macOS, and Linux CI

Not implemented yet: service installation, log rotation, model provider adapters, tools, memory, workflows, and voice.

The next implementation task is the first unchecked item in [TODO.md](../../TODO.md).

## Run It Locally

In one terminal:

```powershell
cargo run -p jarvisd
```

In another:

```powershell
cargo run -p jarvis-cli -- status
cargo run -p jarvis-cli -- health --json
cargo run -p jarvis-cli -- logs --lines 20
cargo run -p jarvis-cli -- doctor
cargo run -p jarvis-cli -- service
```

`jarvis status` and `jarvis health` connect to the same profile the daemon owns. `jarvis logs`, `jarvis doctor`, and `jarvis service` all work without the daemon, which is the point: they must diagnose a broken installation rather than depend on it.

### Portable Mode

Pass an absolute root to keep everything inside one directory:

```powershell
cargo run -p jarvisd -- --root C:\jarvis-portable
cargo run -p jarvis-cli -- status --root C:\jarvis-portable
```

Portable mode creates no service, PATH, or registry change; every managed file stays
inside the root. A relative root is refused rather than resolved against the current
directory. Profiles in different roots cannot collide: the local endpoint is derived
from the root as well as the profile name, so a portable `default` and a native
`default` can run at the same time without contending for one socket or pipe.

`jarvis service` reports what a per-user service reconcile would do. It is read-only:
it prints the definition path and launch command, classifies the existing definition
as `absent`, `current`, `drifted`, or `foreign`, and refuses in portable mode. A
`foreign` definition is never replaced — that requires explicit operator action.

`jarvis doctor` never changes anything unless `--repair` is passed, and any repair is re-verified before its result is reported. Its exit code tells automation what happened: `0` passed, `6` warnings, `7` failed, `8` a repair did not verify. A profile that has never been started reports **warnings**, not success, because there is no daemon, credential, or log record yet.

The daemon prints its readiness line at startup and transitions through `booting` to `ready`; stop it with Ctrl+C, which performs a graceful shutdown rather than an abrupt exit.

Logs are one JSON object per line at `<logs>/jarvisd.jsonl`. Every line passes through the redactor before it is written, so known secrets, credential-shaped key/value text, `Bearer` values, and URL userinfo passwords are masked even if a call site forgets a rule. Lines longer than 8 KiB are truncated on a UTF-8 boundary and report the omitted byte count.

The CLI exits `0` on success, `2` for a usage error, `3` when the daemon is unreachable or not ready, `4` when authentication or authorization fails, and `5` for a rejected request.

The daemon issues a 32-byte credential for its profile on first run and stores it beside the profile configuration. Clients only ever read that value; if it is missing, `jarvis` reports that the daemon has not issued one and exits `3` rather than creating a value. Otherwise any local process could choose the secret the daemon trusts. Do not copy the credential between machines or profiles.

## Orientation

Read in order:

1. [AGENTS.md](../../AGENTS.md)
2. [Product requirements](../product/requirements.md)
3. [Architecture overview](../architecture/overview.md)
4. [Repository layout](../architecture/repository-layout.md)
5. [Roadmap](../../ROADMAP.md)
6. [TODO.md](../../TODO.md)
7. [Testing](testing.md)
8. [Definition of done](definition-of-done.md)

For an external integration, also read [external-research.md](external-research.md) and invoke the repository's `external-integration-research` skill when supported.

## Phase 1 Prerequisites

The first implementation slice should require only:

- Git
- a Rust toolchain installed through rustup
- platform C/C++ build prerequisites required by selected Rust crates
- PowerShell on Windows or a POSIX shell on macOS/Linux

The committed [rust-toolchain.toml](../../rust-toolchain.toml) is the exact Rust version authority. Rustup installs or selects that toolchain automatically when commands run from this repository.

Node.js is introduced only when the web/Tauri UI begins. Python is introduced only for isolated Python runtime workers or the reference prototype. Neither is a core runtime prerequisite.

## Foundation: `P1-001`

The first slice created the smallest compilable workspace:

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
rustfmt.toml                    only if non-default policy is justified
deny.toml                       when dependency policy is enabled
apps/jarvisd/
apps/jarvis-cli/
crates/jarvis-core/
crates/jarvis-application/
crates/jarvis-protocol/
crates/jarvis-storage/
```

The initial binaries printed package versions and exited. They did not contain fake model or tool implementations. Package manifests encode the dependency direction documented in [repository-layout.md](../architecture/repository-layout.md).

Before adding dependencies:

1. Name the use and owning crate.
2. Prefer established workspace dependencies with narrow features.
3. Check license and advisories.
4. Keep domain crates free of framework/provider dependencies.

The root [Cargo.toml](../../Cargo.toml) owns the workspace lint policy. The pinned default rustfmt style applies; Rust unsafe code is forbidden, unused results are denied, public documentation is warned, and Clippy's `all` and `pedantic` groups run with selected placeholder and panic shortcuts denied.

Required baseline after `P1-001`:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Add CI in `P1-002`; do not overload the first slice with service installation, SQL, HTTP, or models.

## Implementation Loop

For each TODO item:

1. Identify the owning module and nearest acceptance test.
2. Read only enough surrounding code/docs to state a falsifiable hypothesis.
3. Make the smallest coherent behavior change.
4. Run the narrow test immediately.
5. Repair that slice before expanding scope.
6. Run package/workspace quality gates.
7. Update docs and the TODO checkbox only after evidence passes.

Keep a runnable daemon/client path from Phase 1 onward. Libraries that no binary exercises are not integrated.

## Configuration And Local Data

Never write local state into the repository. Use OS application directories through the Phase 1 path service. Tests use isolated temporary profiles.

Future environment variables follow a documented prefix such as `JARVIS_`; sensitive variables contain secret values only when the deployment explicitly selects environment-backed secrets. Examples use placeholders and `.env.example`, never `.env`.

The Phase 1 configuration is `<config>/config.toml`. Precedence is built-in defaults, then the file, then the explicit environment allowlist. Schema v1 is:

```toml
schema_version = 1

[profile]
name = "default"

[logging]
level = "info"

[daemon]
shutdown_timeout_seconds = 15
```

Allowed overrides are `JARVIS_PROFILE_NAME`, `JARVIS_LOG_LEVEL`, and `JARVIS_SHUTDOWN_TIMEOUT_SECONDS`. Unknown TOML keys and unknown `JARVIS_*` variables are errors. Missing files use defaults without writing; schema v0 is migrated in memory and must be saved explicitly. Newer schemas fail closed. Configuration writes validate first, replace atomically, and retain no secret values.

## Prototype

The [example](../../example/readme.md) can be run separately to study behavior, subject to its dependencies, terms, and local credential handling. Do not run it automatically during production setup or tests. Do not read or commit its `config/api_keys.json` or `memory/long_term.json`.

Use [the migration inventory](../migration/python-prototype.md) to translate behaviors into clean-room tests and Rust ownership.

## Before You Stop

- report commands actually run and their result
- distinguish implemented, partial, researched, and unverified work
- leave no required server/watch process running unintentionally
- leave the first incomplete TODO task explicit
- do not commit unless the user asks