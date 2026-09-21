---
integration: rust-structured-logging
status: implemented
last_verified: 2026-09-21
owners: []
selected_spec_version: tracing 0.1.44 / tracing-subscriber 0.3.23
selected_sdk: tracing-subscriber 0.3.23
---

# Rust Structured Logging, Correlation, And Redaction

## Scope

The `tracing` ecosystem used for the daemon's structured logs: crate and feature selection, JSON file output, human console output derived from the same event, the filter, the redaction sink, and the bounded log reader behind `jarvis logs`. Metrics, traces, OpenTelemetry export, and audit receipts are out of scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | not found for the tracing crates | 2026-09-21 | discovery |
| `tracing` crate docs | https://docs.rs/tracing/0.1.44/tracing/ | 2026-09-21 | event/span API, feature flags |
| `tracing-subscriber` crate docs | https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/ | 2026-09-21 | layers, `fmt`, JSON, feature flags |
| `cargo search` | tracing 0.1.44, tracing-subscriber 0.3.23, tracing-appender 0.2.5 | 2026-09-21 | current published versions |
| changelog/release notes | `Cargo.lock` resolved versions | 2026-09-21 | compatibility |

No `llms.txt` exists for these crates. The normative contract is the official rustdoc for the selected version.

## Verified Contract

### Crate And Feature Selection

- `tracing = "=0.1.44"` was already resolved transitively through SQLx, so declaring it directly pinned the existing version rather than adding a second one.
- `tracing-subscriber = "=0.3.23"` with `default-features = false` and features `ansi`, `env-filter`, `fmt`, `json`, `registry`, `std`.
- `tracing-appender` (0.2.5) is deliberately **not** used: a background writer thread would need careful shutdown ordering, and the requirement is a bounded, greppable, redacted file rather than high-throughput rotation.

### Documented Feature-Flag Behaviour

From the 0.3.23 rustdoc, `default-features = false` removes `ansi`. `fmt::layer().with_ansi(true)` is a **runtime assertion**, not a compile-time one, so omitting the feature compiles cleanly and panics when the layer is first built:

```text
tracing-subscriber: the `ansi` crate feature is required to enable ANSI terminal colors
```

This was observed live before being fixed. `ansi` is therefore required for the human console layer.

### Output Shape

- File: one JSON object per line (`fmt::layer().json().with_ansi(false)`), which is directly parseable and stable for `jarvis logs`.
- Console: `fmt::layer().with_ansi(true)` to stderr.
- Both layers share one redacting writer, so the two renderings cannot diverge and a secret cannot leak through only one of them.
- Filter: `EnvFilter::try_new(level.as_str())` from the validated `jarvis_core::LogLevel`.

### Limits And Failure Semantics

- `MAX_LINE_BYTES = 8 KiB` per record, truncated on a UTF-8 boundary with an explicit omitted-byte count.
- `MAX_TAIL_LINES = 5_000`, `DEFAULT_TAIL_LINES = 200`.
- `read_tail` reads backwards in 64 KiB windows and rejoins lines split across a window boundary, so a large log is never fully loaded into memory.
- A missing log is an empty result, not an error; `jarvis logs` exits 0 with an explanatory message.

## JARVIS Mapping

- `jarvis_core::LogLevel` is the single definition of the level vocabulary; `jarvis-storage` (config) and `jarvis-observability` (logging) both reference it instead of the adapter crates depending on each other.
- `jarvis_observability::Redactor` masks known secrets, credential-shaped key/value and query text, `Bearer` values, and URL userinfo passwords.
- The daemon builds the redactor from the profile credential before logging starts, so the credential is masked from the first recorded line.

## Decisions

- Redaction is applied in the **writer**, not at call sites, so a forgotten rule cannot leak. A call site can still avoid constructing a sensitive field, which remains the stronger layer.
- Two layers (JSON file, human console) share one redactor rather than formatting separately.
- `jarvis logs` reads the file directly instead of asking the daemon, because logs must remain diagnosable precisely when the daemon cannot start.
- `LogLevel` moved to `jarvis-core`; keeping it in `jarvis-storage` would force the logging adapter to depend on the storage adapter for a five-variant enum.

## Rejected Alternatives

- `tracing-appender` non-blocking writer: adds a background thread and shutdown ordering for no current requirement.
- One formatter for both sinks: the operator wants human output and the tool wants JSON, and deriving both from the same event satisfies the observability rule.
- Redacting only known secret values: a redactor that handles only the credential passes a naive test while leaving API keys in query strings and passwords in DSNs intact. Each class has its own test.

## Evidence Of Implementation

- `crates/jarvis-observability/src/redact.rs` (11 unit tests across the three secret classes, plus over-matching and bounds)
- `crates/jarvis-observability/src/logging.rs` (tail reader, window-boundary correctness, sink redaction proof)
- `crates/jarvis-core/src/loglevel.rs`
- `apps/jarvisd/src/main.rs` (startup/shutdown events, redactor built from the credential)
- `apps/jarvis-cli/src/main.rs` (`jarvis logs`, `--lines`)
- Manual verification: `jarvis logs` returned the structured record; the 64-character credential did not appear anywhere in the log file.

## Unresolved Ambiguities

- Log rotation and retention are not implemented; the file grows until the operator or a future task bounds it.
- The graceful-shutdown `daemon stopped` event is implemented and the signal handlers are unit-tested, but a console close event could not be delivered in the automated environment, so that specific line was not observed end to end.
- Traces, metrics, and audit receipts remain unimplemented.
