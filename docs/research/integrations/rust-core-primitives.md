---
integration: rust-core-primitives
status: implemented
last_verified: 2026-09-20
owners: []
selected_spec_version: RFC 9562 UUIDv7 and RFC 3339 timestamps
selected_sdk: uuid 1.26.1 / time 0.3.55 / serde 1.0.229 / serde_json 1.0.151 / thiserror 2.0.20
---

# Rust Core Primitives

## Scope

This record covers the third-party Rust crates used for Phase 1 typed UUIDv7 identifiers, UTC timestamps, serialization, JSON test/wire encoding, and standard error implementations. Configuration, SQLite, async runtime, CLI, and IPC dependencies are researched separately when selected.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| AI documentation indexes | `docs.rs`, `serde.rs`, `time-rs`, and `uuid-rs` `llms.txt` paths returned no usable index | 2026-09-20 | discovery attempt |
| `uuid` package metadata | https://crates.io/api/v1/crates/uuid (1.26.1) | 2026-09-20 | version, features, license, MSRV |
| `uuid::Uuid` rustdoc | https://docs.rs/uuid/1.26.1/uuid/struct.Uuid.html | 2026-09-20 | UUIDv7 generation, validation, timestamp extraction |
| `time` package metadata | https://crates.io/api/v1/crates/time (0.3.55) | 2026-09-20 | version, features, license, MSRV |
| `OffsetDateTime` rustdoc | https://docs.rs/time/0.3.55/time/struct.OffsetDateTime.html | 2026-09-20 | UTC and nanosecond timestamps |
| RFC 3339 serde rustdoc | https://docs.rs/time/0.3.55/time/serde/rfc3339/index.html | 2026-09-20 | stable human-readable timestamp encoding |
| Serde package/docs | https://crates.io/api/v1/crates/serde (1.0.229), https://serde.rs/derive.html | 2026-09-20 | derived typed serialization |
| Serde JSON package/docs | https://crates.io/api/v1/crates/serde_json (1.0.151), https://docs.rs/serde_json/1.0.151/serde_json/ | 2026-09-20 | strongly typed JSON round trips |
| `thiserror` package/docs | https://crates.io/api/v1/crates/thiserror (2.0.20), https://docs.rs/thiserror/2.0.20/thiserror/ | 2026-09-20 | standard error implementations |

## Verified Contract

### Operations And Transport

`uuid` supports RFC 9562 version 7 generation through `Uuid::now_v7` and deterministic construction through `Uuid::new_v7(Timestamp)`. It exposes `get_version_num` and timestamp extraction for validation. `time::OffsetDateTime` provides UTC construction and nanosecond Unix timestamps; the `serde-well-known` feature enables RFC 3339 field encoding. Serde derives strongly typed data contracts, while `serde_json` provides bounded in-memory test and wire round trips.

### Authentication And Authorization

Not applicable. These libraries perform local computation and do not accept credentials or make network calls at runtime.

### Limits And Failure Semantics

UUID parsing and version validation are fallible. UUIDv7 embeds millisecond timestamp precision even when the surrounding UTC timestamp retains nanoseconds. RFC 3339 parsing and formatting are fallible and reject out-of-range values. JSON decoding is fallible and is later wrapped with protocol frame-size limits. `thiserror` generates only standard `Error`, `Display`, and conversion implementations; safe messages remain a JARVIS responsibility.

### Data And Compliance

No data leaves the process. All five crates are licensed `MIT OR Apache-2.0`. Selected MSRVs range from Rust 1.56 through 1.88, below the repository's Rust 1.98.1 pin. Default `serde_json` ordering is retained; `preserve_order` is not enabled.

### Versions And Deprecations

The selected releases were current, non-yanked crates.io versions on 2026-09-20. Only narrow features are enabled: `uuid` uses its default `std` support plus `v7`; `time` uses `std`, `serde`, and `serde-well-known`; Serde uses `derive`. No unstable features are selected.

## JARVIS Mapping

- `uuid::Uuid` remains private behind non-interchangeable JARVIS ID newtypes.
- `OffsetDateTime` remains private behind `UtcTimestamp` and an injectable `Clock` port.
- Serde is a representation mechanism, not the canonical domain model.
- `thiserror` is implementation machinery and does not determine domain error codes or retryability.
- Secret references contain locator metadata only; none of these crates receives secret values.

## Decisions

- Require UUID version 7 on every typed-ID parse and constructor.
- Use UUIDv7 for application-generated durable and correlation identities.
- Keep nanosecond UTC timestamps and serialize them as RFC 3339.
- Inject clocks and ID generators so tests never rely on wall time or random identity.
- Centralize dependency versions and features in `[workspace.dependencies]`.

## Rejected Alternatives

- Raw `String` identifiers were rejected because they permit accidental cross-entity substitution.
- UUIDv4 was rejected for durable application identities because it lacks time ordering.
- Integer timestamps were rejected as public/domain representations because units are ambiguous.
- `chrono` was not added because `time` supplies the narrower UTC/RFC 3339 behavior required here.
- `anyhow` was not added to domain crates because callers need stable error classes and retryability.

## Verification Plan

- Generate every ID type and assert UUID version 7 plus distinct Rust types.
- Reject parsed UUIDv4 and malformed identifiers.
- Use fixed generators and clocks to prove deterministic output.
- Round-trip timestamps through RFC 3339 JSON and preserve nanoseconds.
- Assert every domain error class has the intended default retryability.
- Insert a canary as a secret-reference locator and prove `Debug` and `Display` redact it.
- Run Clippy, tests, `cargo deny`, and doctests with warnings denied.

The cheapest central-contract test is `cargo test -p jarvis-core`; it directly falsifies version enforcement, timestamp encoding, retryability, and redaction.

## Unresolved Questions

- UUIDv7 ordering across multiple independent processes is not guaranteed by `Uuid::now_v7`; Phase 1 has one daemon writer per profile, so cross-process monotonic allocation is not required.
- Future database backends may choose binary UUID storage. Phase 1 exposes stable text conversion without fixing the storage representation in core.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | uuid 1.26.1, time 0.3.55, serde 1.0.229, serde_json 1.0.151, thiserror 2.0.20 | Initial Phase 1 core dependency selection from current crates.io metadata and rustdoc | GitHub Copilot |