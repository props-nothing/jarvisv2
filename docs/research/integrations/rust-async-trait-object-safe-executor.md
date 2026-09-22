---
integration: rust-async-trait-object-safe-executor
status: decided
last_verified: 2026-09-22
owners: []
selected_spec_version: n/a
selected_sdk: async-trait 0.1.92
---

# `async-trait` For The Object-Safe Tool Execution Port

## Scope

In scope: choosing how a **dynamically dispatched async trait** is expressed for the tool execution
port in `P3-005`, and confirming that no new package enters the dependency graph.

- `jarvis_tools::executor::ToolExecutor` must be usable as `dyn ToolExecutor`, because the pipeline
  holds a set of adapters resolved at runtime rather than one statically known type.
- It must be `Send + Sync` because it is held behind a shared handle across `await` points.
- It must be `async` because an adapter call is I/O (filesystem, HTTP, a child process).

Out of scope: the adapter implementations themselves (`P3-006` onward) and cancellation (`P3-011`).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| crates.io API | `https://crates.io/api/v1/crates/async-trait` | 2026-09-22 | version, license, MSRV, features |
| crate source | `~/.cargo/registry/src/index.crates.io-*/async-trait-0.1.92/` | 2026-09-22 | the actual resolved version, read from the vendored source |
| locked graph | `Cargo.lock` | 2026-09-22 | confirms the version is already resolved |
| Rust reference | the `dyn` compatibility rules for traits with generic methods | 2026-09-22 | why the port is written the way it is |

The contract was read from the **vendored source in `Cargo.lock`'s resolved version** rather than from
a documentation site, which is the method `docs/development/external-research.md` ranks as "official SDK
source". `P3-002` was bitten by reading a version that was not the resolved one (`referencing` 0.33.0
versus 0.57.0 have different constructors), so the rule is applied to every crate now.

## Verified Contract

### Versions And Features

- **0.1.92** is already a workspace dependency and already consumed by `jarvis-models`, so declaring it
  in `jarvis-tools` adds **no new package** and no new lock entry. Reusing an existing locked dependency
  is the same selection argument `P2-007a` used for `axum` and `P3-004` used for `sha2`.
- **MIT OR Apache-2.0**, both already in `deny.toml`'s allow list.
- **No features**: the crate is a proc-macro with none declared, so nothing to disable.
- `async_trait` is a proc-macro that rewrites a method returning a future into a method returning
  `Pin<Box<dyn Future<...>>>`, which is what makes the trait object-safe.

### Properties Relied On

- **Object safety.** A trait with an `async fn` is not object-safe in stable Rust; `#[async_trait]`
  desugars the method to a returned boxed future, which is. This is the whole reason for the dependency.
- **`Send + Sync` bounds are declared explicitly** on the trait (`Send + Sync` on the trait itself), and
  `#[async_trait]` then requires the returned future to be `Send`. A future that is not `Send` fails to
  compile rather than failing at runtime in a multi-threaded runtime.
- **The boxed future is an allocation per call.** Accepted: a tool call is dominated by I/O and an
  audit write, so one allocation is not measurable, and the alternative costs a generic parameter
  through every layer that holds an adapter.

## JARVIS Mapping

- `jarvis_tools::executor::ToolExecutor` is declared with `#[async_trait]` and a manual
  `impl fmt::Debug for dyn ToolExecutor`, because `dyn Trait` does not inherit an implementation from the
  trait's own `Debug` supertrait. The `adapter_id()` method exists partly for that reason: it is what
  the `Debug` implementation can print, and it is the field an audit record can attribute a call to.
- `getrandom` is the other dependency this slice adds to `jarvis-tools`, and its contract is already
  recorded in `docs/research/integrations/local-daemon-protocol.md` (0.4.3, entropy for credentials).
  `P3-005`'s use is the same kind of use — 16 random bytes for an idempotency key — so the existing
  record covers it rather than a second record restating the same contract.
- No provider SDK type appears in the port. `AdapterError` and `ToolCallResult` are JARVIS types, so a
  vendor error cannot cross the domain boundary, which `AGENTS.md` requires.

## Unresolved Or Worth Watching

- `async-trait` boxes every call. If the tool pipeline becomes allocation-sensitive, the alternative is
  a hand-written `fn execute(&self, ...) -> Pin<Box<dyn Future<Output = ...> + Send + '_>>` with no
  macro. That is a mechanical change and is not taken now, because there is no measurement to justify it.
- Rust's native `async fn` in traits (stabilized after this project's toolchain baseline) does not
  make `dyn` dispatch work, so this dependency cannot be dropped by writing the trait natively. It can
  only be dropped by hand-writing the boxed return type.
- The `Debug` implementation for `dyn ToolExecutor` prints only `adapter_id()`. If a call needs more
  attribution, the field belongs on `ToolExecutionRequest` rather than in the adapter.

## Falsifying Test

`the_port_is_object_safe_and_reports_an_outcome` in `crates/jarvis-tools/src/executor.rs` drives a
`Box<dyn ToolExecutor>` through `#[tokio::test]` and asserts the result, so object safety and
runtime-driveability are proved by running rather than asserted in prose. The falsification is a change
to the trait that removes object safety: the test file then fails to compile.
