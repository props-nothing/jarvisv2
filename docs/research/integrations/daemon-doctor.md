---
integration: daemon-doctor
status: implemented
last_verified: 2026-09-21
owners: []
selected_spec_version: 1
selected_sdk: sqlx 0.9.0
---

# Offline `jarvis doctor`: Detection, Evidence, Repair, And Verification

## Scope

The offline diagnostic path that explains why a local JARVIS installation is not
working: read-only inspection of configuration, paths, the SQLite database, the
profile credential, the daemon lock, protocol alignment, redaction, and logs;
stable finding codes; explicit repair; and post-repair verification. Live probing
of a running daemon, service drift, ports, and update state are out of scope for
this record.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | not found for SQLx, SQLite, or the Rust standard library | 2026-09-21 | discovery |
| SQLx `SqliteConnectOptions` | https://docs.rs/sqlx/0.9.0/sqlx/sqlite/struct.SqliteConnectOptions.html | 2026-09-21 | read-only connection options |
| SQLite `PRAGMA` reference | https://sqlite.org/pragma.html | 2026-09-21 | `application_id`, `user_version`, `integrity_check`, `foreign_key_check`, `journal_mode` |
| SQLite file format | https://sqlite.org/fileformat2.html | 2026-09-21 | byte offsets for the header fields used in the live test |
| Rust `File::try_lock` / `TryLockError` | https://doc.rust-lang.org/std/fs/enum.TryLockError.html | 2026-09-21 | probing the daemon lock without stealing it |

Repository-internal normative sources: `docs/operations/install-and-release.md`
(Doctor contract), `docs/quality/acceptance-tests.md` A02, `docs/architecture/storage.md`,
`docs/operations/observability.md`, `docs/research/upstream-patterns.md`.

## Verified Contract

### Required Output Per Check

From `docs/operations/install-and-release.md`: a stable ID, severity, facts,
remediation, whether repair is safe, and post-repair verification; `--json` must be
suitable for support automation. From A02: a stable finding code, safe evidence, a
specific remediation, **no secret leakage**, and **no mutation unless repair was
explicitly requested**.

### The Decisive Constraint: Doctor Must Not Mutate

`SqliteDatabase::open` migrates, creates the file when absent, and enables WAL, so
it cannot be used for diagnosis. Inspection therefore uses
`SqliteConnectOptions::new().read_only(true).create_if_missing(false)` and returns a
newer schema as *data* (`DatabaseState::FutureSchema`) rather than as an error.

Measured on the live profile (`%LOCALAPPDATA%\JARVIS\jarvis.sqlite3`):

1. A healthy installation reported `outcome=warnings` (8 info, 1 warning), exit 6.
2. Writing `99` into the `user_version` header field (offset 60, big-endian) made
   `doctor` report `database.schema_too_new` with `found`/`supported` evidence and a
   concrete remediation, exit 7.
3. `jarvisd` refused the same file: `SQLite schema 1660944386 is newer than
   supported schema 2`, exit 1. The guard and the explanation agree.
4. The database file hash was **identical before and after** the doctor run, so the
   diagnostic path itself mutated nothing.

### Implemented Checks

| Check | Codes | Notes |
| --- | --- | --- |
| configuration | `config.valid`, `config.invalid`, `config.schema_too_new` | unknown keys and future schema are distinct |
| paths | `paths.private`, `paths.insecure`, `paths.unavailable` | Unix mode check; Windows ACLs not measured |
| database | `database.absent`, `database.current`, `database.migration_pending`, `database.schema_too_new`, `database.schema_inconsistent`, `database.foreign`, `database.unreadable`, `database.integrity_failed` | read-only |
| credential | `credential.present`, `credential.missing`, `credential.invalid` | value never leaves the store |
| daemon | `daemon.running`, `daemon.not_running` | kernel lock ownership via `try_lock` |
| protocol | `protocol.aligned`, `protocol.mismatch`, `protocol.unreachable` | from build constants |
| redaction | `redaction.self_test_passed`, `redaction.self_test_failed` | canary through the real redactor |
| logs | `logs.readable`, `logs.empty` | bounded reader |

### Limits And Failure Semantics

- Report outcome → exit code: `0` passed, `6` warnings, `7` failed, `8` repair did
  not verify. Distinct codes let support automation branch without parsing text.
- `jarvis doctor` exits **6**, not 0, on a profile that has never started, because
  there is no daemon, no credential, and no log record yet. Reporting `passed` would
  be a false reassurance.
- Evidence values are flattened and capped at 400 bytes, so a finding stays
  single-line and cannot become a smuggling channel.
- Redaction self-test failure is a blocking error, not a warning.

## JARVIS Mapping

- `jarvis_storage::inspect_database` → `DatabaseInspection`, `DatabaseState`
- `jarvis_storage::CredentialStore::inspect_presence` → presence without the value
- `jarvis_diagnostics::{diagnose, repair, Report, Finding, FindingCode, Severity}`
- CLI surface: `jarvis doctor [--json] [--repair]`

## Decisions

- Doctor lives in a new `jarvis-diagnostics` crate rather than in `jarvis-cli`.
  `docs/architecture/repository-layout.md` forbids the CLI from touching the
  database, yet doctor must inspect it offline before any daemon exists. A separate
  crate preserves that rule instead of carving an exception into the CLI.
- Repair authority is limited to idempotent, JARVIS-owned directory permissions.
  Nothing that destroys data, moves a database, or changes version compatibility is
  ever automatic.
- Post-repair verification calls the **same** detection function that failed, so a
  repair cannot pass a weaker check than the one that reported the problem.
- The daemon lock is probed with `try_lock` and immediately unlocked, because kernel
  lock ownership is the authority and the file text is only diagnostic.

## Rejected Alternatives

- Reusing `SqliteDatabase::open` for diagnosis: it migrates, creates, and enables WAL.
- Treating a newer schema as a hard error in doctor: the state must be explained, not
  refused.
- Implementing doctor inside the CLI: violates the CLI ownership rule.
- Inferring "daemon running" from lock-file existence or its PID text: both are
  wrong after an unclean exit.
- Claiming `paths.private` on Windows without measuring ACLs: doctor reports no
  verdict rather than one it did not measure.
- Auto-repairing a broken configuration or schema: those repairs are destructive and
  must be operator-initiated.

## Evidence Of Implementation

- `crates/jarvis-storage/src/inspect.rs` — 5 tests, including a no-mutation proof
- `crates/jarvis-diagnostics/src/findings.rs` — code uniqueness, namespacing, and a
  test asserting no error-severity code claims "no action required"
- `crates/jarvis-diagnostics/src/doctor.rs`, `repair.rs`
- `crates/jarvis-diagnostics/tests/a02_broken_install.rs` — 7 acceptance tests
  covering newer schema, repair-does-not-touch-database, corrupt database, invalid
  configuration, never-started profile, secret non-leakage, and severity ordering
- `apps/jarvis-cli/src/main.rs`, `output.rs`

## Unresolved Ambiguities

- Doctor does not probe a running daemon over the local protocol, so an unhealthy but
  running daemon is reported only as `daemon.running`.
- Windows ACL privacy is unmeasured, so `paths.insecure` cannot fire on Windows yet.
- Service-definition drift, occupied ports, disk space, clock skew, and
  update/rollback state are not yet checks.
- Repair covers directory permissions only; it never touches configuration, schema,
  or credentials.
