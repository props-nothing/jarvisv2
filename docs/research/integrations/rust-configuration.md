---
integration: rust-configuration
status: implemented
last_verified: 2026-09-20
owners: []
selected_spec_version: TOML 1.1.0 / JARVIS configuration schema v1
selected_sdk: toml 1.1.6+spec-1.1.0 / atomic-write-file 0.3.1 / serde 1.0.229
---

# Rust Configuration

## Scope

This record covers Phase 1 parsing, validation, environment overrides, migration, and atomic persistence of the local JARVIS configuration file. Secret-value loading, model/provider configuration, service arguments, and remote configuration are out of scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| AI documentation indexes | `toml.io/llms.txt` had no usable extracted index; docs.rs crate `llms.txt` paths returned no usable index | 2026-09-20 | discovery attempt |
| TOML specification | https://toml.io/en/v1.1.0 (1.1.0) | 2026-09-20 | UTF-8 syntax, key/table uniqueness, data model |
| Versioned TOML specification source | https://github.com/toml-lang/toml/blob/1.1.0/toml.md | 2026-09-20 | tagged normative text |
| `toml` package metadata | https://crates.io/api/v1/crates/toml (1.1.6+spec-1.1.0) | 2026-09-20 | version, features, license, Rust 1.85 MSRV |
| `toml` rustdoc | https://docs.rs/toml/1.1.6+spec-1.1.0/toml/ | 2026-09-20 | typed parsing, `Table`, and pretty serialization |
| `toml` release tag | https://github.com/toml-rs/toml/releases/tag/toml-v1.1.6 | 2026-09-20 | selected release revision |
| Serde container attributes | https://serde.rs/container-attrs.html | 2026-09-20 | `deny_unknown_fields` behavior |
| `atomic-write-file` package metadata | https://crates.io/api/v1/crates/atomic-write-file (0.3.1) | 2026-09-20 | version, features, BSD-3-Clause license, Rust 1.85 MSRV |
| Atomic writer rustdoc | https://docs.rs/atomic-write-file/0.3.1/atomic_write_file/ | 2026-09-20 | same-directory temporary file, fsync, replace, cleanup, platform limitations |
| Atomic writer release | https://github.com/andreacorbellini/rust-atomic-write-file/releases/tag/v0.3.1 | 2026-09-20 | `rand` 0.10 and Unix `nix` 0.31 updates |

## Verified Contract

### Format And Operations

TOML 1.1 is case-sensitive UTF-8 and maps unambiguously to tables. Duplicate keys and duplicate table definitions are invalid. `toml` supports typed Serde parsing, table inspection, and pretty serialization. Serde's `deny_unknown_fields` rejects keys that would otherwise be ignored.

`atomic-write-file` 0.3.1 writes a temporary file in the destination directory, synchronizes it, and replaces the destination only on `commit`. Dropping or explicitly discarding before commit leaves the previous file unchanged; an abrupt process stop can leave a temporary file. Unix mode can be selected and preservation disabled. Non-Unix ownership, permissions, and ACLs are not preserved by the crate.

### Authentication And Secrets

No network authentication or scopes apply. Phase 1 configuration contains no secret values. Future credentials remain `SecretRef` metadata or explicit environment-backed secret adapters. Config parse/write errors never reproduce document values or full local paths.

### Limits And Failure Semantics

The configuration document is bounded to 1 MiB and must be UTF-8. Syntax/type failures, unknown keys, unsupported versions, invalid values, symlink endpoints, and I/O failures are distinct safe errors. A newer schema fails closed and is never rewritten. Writes validate fully before opening the atomic writer.

Atomic replacement does not provide multi-process compare-and-swap. Phase 1's singleton daemon is the sole normal writer; later UI/CLI changes must go through that daemon. The configuration directory and final file are re-hardened through the Phase 1 path service after replacement, including Windows ACLs.

### Versions And Compliance

`toml` 1.1.6 is `MIT OR Apache-2.0`; `atomic-write-file` 0.3.1 is `BSD-3-Clause`; Serde is `MIT OR Apache-2.0`. Their MSRVs are below Rust 1.98.1. Default TOML parse/Serde/display support is selected. No unstable atomic-writer feature is enabled.

## JARVIS Mapping

- `jarvis-storage` owns configuration file I/O, migration, and atomic persistence.
- The durable file is `<config>/config.toml` and always serializes the current schema version.
- Schema v1 contains `profile.name`, `logging.level`, and `daemon.shutdown_timeout_seconds`.
- The explicit v0 flat shape exists only as a tested migration boundary: `profile`, `log_level`, and `shutdown_timeout_seconds`.
- Environment precedence is defaults, then file, then an explicit `JARVIS_*` allowlist.
- Allowed overrides are `JARVIS_PROFILE_NAME`, `JARVIS_LOG_LEVEL`, and `JARVIS_SHUTDOWN_TIMEOUT_SECONDS`.
- Any other `JARVIS_*` key fails to catch misspellings; unrelated environment keys are ignored.
- Loading a v0 document returns migration evidence but does not mutate disk. The daemon performs an explicit validated save when migration is accepted.

## Decisions

- Use typed TOML, not ad hoc line parsing or untyped runtime lookups.
- Reject unknown fields at every schema level.
- Refuse missing, malformed, negative, or newer schema versions.
- Keep environment overrides non-secret and fully enumerated.
- Use same-directory atomic replacement; never remove the old file before rename.
- Reject symlink config endpoints before read or write.
- Enforce Unix mode `0600` and current-user Windows ACLs on existing and replacement files.
- Surface migration provenance separately from the effective configuration.

## Rejected Alternatives

- JSON was rejected for the human-edited local configuration because TOML offers comments and clearer section structure.
- YAML was rejected because its broader type/coercion surface is unnecessary.
- `config` and `figment` were rejected because three explicit overrides do not justify a larger merge framework, and JARVIS needs strict unknown-key/version control.
- Manual temporary-write plus `std::fs::rename` was rejected because replacement semantics differ on Windows and delete-then-rename creates a missing-file window.
- Automatic disk mutation during load was rejected because inspection and `doctor` must not mutate without an explicit repair/migration action.

## Verification Plan

- Parse schema v1 and round-trip through pretty TOML.
- Reject unknown root and nested keys, malformed TOML, invalid values, missing versions, and unsupported newer versions.
- Migrate an explicit v0 fixture and assert exact v1 values plus migration evidence.
- Prove allowed environment values override the file and unrelated keys do not.
- Reject an unknown `JARVIS_*` variable and a non-Unicode recognized value.
- Save twice and prove the second valid document fully replaces the first with no partial content.
- Seed a valid file, force a pre-commit write failure through an injected test writer, and prove the old bytes remain.
- Reject a symlink endpoint.
- Verify Unix file mode `0600` and, on Windows, absence of an `Everyone` ACE after save.
- Run native Windows, macOS, and Linux tests plus workspace quality and dependency gates.

The cheapest central-contract test is `cargo test -p jarvis-storage config`; it directly falsifies strict schema parsing, migration, precedence, and atomic replacement.

## Unresolved Questions

- Comment preservation is not provided by typed reserialization. Phase 1 writes only explicit migration/default updates; a future settings editor may need `toml_edit` if preserving user formatting becomes a requirement.
- Concurrent external editors can race the singleton daemon's save. File watching and optimistic config revisions are deferred until an actual settings workflow exists.
- Environment-backed secret values require a separate explicit deployment mode and are not enabled by this schema.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | TOML 1.1.0, toml 1.1.6, atomic-write-file 0.3.1, serde 1.0.229 | Initial Phase 1 selection; atomic writer 0.3.1 updates `rand` and Unix `nix` dependencies | GitHub Copilot |