---
integration: sqlite-sqlx
status: implemented
last_verified: 2026-09-20
owners: [storage]
selected_spec_version: SQLite 3.51.3
selected_sdk: SQLx 0.9.0; libsqlite3-sys 0.37.0; Tokio 1.53.1
---

# SQLite, SQLx, And Tokio

## Scope

This record covers local SQLite connection setup, embedded migrations, schema
compatibility checks, write-ahead logging, and a verified backup before an
existing database is upgraded. Remote databases, replication, encryption at
rest, SQLCipher, and PostgreSQL are out of scope for `P1-006`.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | `sqlx.dev/llms.txt`, `docs.rs/sqlx/llms.txt`, `tokio.rs/llms.txt`, and `sqlite.org/llms.txt` not found | 2026-09-20 | discovery attempt |
| SQLx crate docs | https://docs.rs/sqlx/0.9.0/sqlx/ | 2026-09-20 | runtime and feature contract |
| SQLx SQLite options | https://docs.rs/sqlx/0.9.0/sqlx/sqlite/struct.SqliteConnectOptions.html | 2026-09-20 | connection pragmas and defaults |
| SQLx migrator | https://docs.rs/sqlx/0.9.0/sqlx/migrate/struct.Migrator.html | 2026-09-20 | locking, checksums, and embedded migrations |
| SQLx release notes | https://github.com/transact-rs/sqlx/blob/v0.9.0/CHANGELOG.md | 2026-09-20 | compatibility and SQLite feature changes |
| SQLx package metadata | https://crates.io/crates/sqlx/0.9.0 | 2026-09-20 | version, license, MSRV, and features |
| SQLite WAL | https://sqlite.org/wal.html | 2026-09-20 | WAL behavior and corruption fix boundary |
| SQLite 3.51.3 release | https://sqlite.org/releaselog/3_51_3.html | 2026-09-20 | WAL-reset fix and release identity |
| SQLite pragmas | https://sqlite.org/pragma.html | 2026-09-20 | integrity, foreign keys, trusted schema, and durability |
| SQLite backup API | https://sqlite.org/backup.html | 2026-09-20 | consistent live-copy behavior |
| SQLite `VACUUM INTO` | https://sqlite.org/lang_vacuum.html#vacuuminto | 2026-09-20 | application-level backup operation |
| `libsqlite3-sys` metadata and source | https://crates.io/crates/libsqlite3-sys/0.37.0 | 2026-09-20 | bundled SQLite selection |
| Tokio crate docs | https://docs.rs/tokio/1.53.1/tokio/ | 2026-09-20 | runtime features and supported platforms |
| Tokio package metadata | https://crates.io/crates/tokio/1.53.1 | 2026-09-20 | version, license, and MSRV |

## Verified Contract

### Operations And Transport

SQLite is an in-process database. SQLx 0.9.0 supports SQLite pools when a
runtime feature is enabled and embeds migrations with `migrate!`. The migrator
locks by default, records applied migrations, verifies checksums for previously
applied migrations, and rejects missing applied versions unless explicitly
configured otherwise.

`SqliteConnectOptions` supports an OS path directly, explicit file creation,
foreign-key enforcement, busy timeout, journal mode, synchronous mode, and
custom initial pragmas. WAL mode persists in the database header. The selected
configuration uses WAL, `synchronous=FULL`, foreign keys enabled, a bounded busy
timeout, and `trusted_schema=OFF` on every connection.

`VACUUM INTO` creates a consistent, compact snapshot in a new destination file
and does not overwrite an existing file. A backup is accepted only after the
copy independently returns `ok` from `PRAGMA integrity_check` and no rows from
`PRAGMA foreign_key_check`.

### Authentication And Authorization

No network authentication, token, scope, callback, or webhook exists. Access is
controlled by the per-user data directory and database-file permissions. The
database, backup, WAL, and shared-memory files must remain under a directory
whose ACL or mode permits only the current user.

### Limits And Failure Semantics

SQLite permits one writer at a time. WAL allows concurrent readers and a writer,
but can still return `SQLITE_BUSY`; connections therefore use a finite busy
timeout and surface exhaustion as an error rather than retrying indefinitely.
Migrations and backups are not silently retried after an ambiguous I/O error.

The WAL-reset corruption bug affects SQLite 3.7.0 through 3.51.2 and is fixed in
3.51.3. SQLx 0.9.0 accepts `libsqlite3-sys >=0.30.1,<0.38.0`; version 0.37.0 is
the highest compatible release and its normal bundled amalgamation identifies
itself as SQLite 3.51.3. JARVIS pins that binding and verifies `sqlite_version()`
at runtime before enabling WAL.

### Data And Compliance

No data leaves the device. SQLite receives canonical local JARVIS state, which
may later contain personal data and secrets by reference. Structural errors may
include bounded operation context but must not include row contents. Backups
inherit the source data classification and user-only permissions. Storage cost
is local disk usage; an upgrade temporarily requires enough free space for a
second compact database image.

### Versions And Deprecations

- SQLx 0.9.0 is stable, MIT OR Apache-2.0, and requires Rust 1.94.0.
- Tokio 1.53.1 is stable, MIT, and requires Rust 1.71.
- `libsqlite3-sys` 0.37.0 is MIT and bundles SQLite 3.51.3.
- SQLite 3.51.3 was released 2026-03-13 and fixes the WAL-reset bug.
- The workspace uses Rust 1.98.1, satisfying all selected MSRVs.
- SQLx 0.9 made SQLite extension loading an explicit unsafe feature. JARVIS does
  not enable the `sqlite` umbrella or any extension-loading feature.

## JARVIS Mapping

The database path is an absolute child of `AppPaths::data()`. SQLx and SQLite
types stay inside `jarvis-storage`; callers receive a storage handle and bounded
storage errors. SQLite's application ID, user version, and SQLx migration table
form the local compatibility evidence. A database from a newer schema version,
an unmarked non-empty database, a changed migration checksum, a corrupt backup,
or a runtime SQLite older than 3.51.3 fails closed before normal operation.

This local storage operation has no tool effect, model scope, or external audit
scope. Startup diagnostics may report schema versions and a backup filename,
but never record stored rows.

## Decisions

- Pin SQLx 0.9.0, Tokio 1.53.1, and `libsqlite3-sys` 0.37.0.
- Enable only SQLx `runtime-tokio`, `sqlite-bundled`, `migrate`, and `macros`.
- Use embedded, monotonic, transactional migrations and retain SQLx's default
  migration locking and checksum validation.
- Mark JARVIS databases with an application ID and application-owned schema
  version in addition to SQLx migration history.
- Back up only an existing, recognized database with pending migrations, then
  verify the backup independently before applying any migration.
- Keep WAL durability at `synchronous=FULL`; local canonical state is not a
  cache for which power-loss durability can be traded away.

## Rejected Alternatives

- System SQLite is rejected because its version and compile options vary across
  installations and cannot guarantee the WAL fix.
- SQLx's `sqlite` umbrella feature is rejected because it enables extension
  loading and other capabilities not needed by JARVIS.
- A raw filesystem copy is rejected because the database may have committed WAL
  content not present in the main file and a concurrent copy need not be a
  consistent snapshot.
- Blind migration of any SQLite file is rejected because it could modify an
  unrelated application database selected by a bad path.
- `synchronous=NORMAL` is rejected because committed transactions can be lost
  after power loss in WAL mode.

## Verification Plan

- Open a fresh absolute path, run embedded migrations, and assert application
  ID, user version, migration history, foreign keys, trusted schema, WAL, and
  `synchronous=FULL`.
- Reopen idempotently and prove no backup is created when no migration is
  pending.
- Build a recognized prior-version fixture, insert sentinel data, upgrade it,
  verify a backup was created before migration, and restore the backup to prove
  the sentinel and prior schema remain readable.
- Reject a future user version and an unmarked non-empty SQLite database without
  changing either file.
- Corrupt or obstruct a backup destination and prove migration does not begin.
- Query the runtime SQLite version and require 3.51.3 or newer.

The cheapest test that disproves the central assumption opens a fresh database
through the production setup function and checks both `sqlite_version()` and
the complete connection/schema pragma contract.

## Unresolved Questions

No unresolved question blocks `P1-006`. Encryption at rest and online backup
retention policy are intentionally deferred and require their own architecture
decision before they can change this storage contract.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | SQLx 0.9.0; `libsqlite3-sys` 0.37.0; SQLite 3.51.3; Tokio 1.53.1 | Selected bundle contains the first WAL-reset fix; fresh, reopen, upgrade, restore, future-schema, foreign-schema, and invalid-backup tests pass | GitHub Copilot |