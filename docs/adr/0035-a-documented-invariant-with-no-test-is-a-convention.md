# ADR-0035: A documented invariant with no test is a convention

**Status:** Accepted

**Date:** 2026-09-23

## Context

`docs/architecture/repository-layout.md` states, of `jarvis-mcp-transport`:

> No provider SDK type appears in this crate's public surface either — `AGENTS.md` forbids SDK types crossing a
> JARVIS boundary, and an error type is a boundary.

`P3-009b` made three public functions that returned `StreamableHttpServerConfig`,
`StreamableHttpService<JarvisMcpServer, NeverSessionManager>`, and (through them) `NeverSessionManager`. Every
one type-checked. Nothing failed. The documentation had said not to, and **a sentence is not a guard**.

So a test was written to enforce it by reading this crate's own sources and reporting any public declaration
whose signature names an SDK type. Writing that test produced three findings, in increasing order of
unpleasantness:

1. **It did not catch the violation it was written for.** The first version searched for the literal crate name
   `rmcp`, which finds a *fully-qualified* path (`rmcp::transport::IntoTransport`) and misses an *imported*
   name. `pub fn sdk(&self) -> StreamableHttpServerConfig` was therefore reported as clean. Found by
   **falsifying**: restoring the public signature left the test green.
2. **The invariant was already false, in three places, from two earlier slices.** `connect_over` (`P3-008e`) is
   public with `rmcp::transport::IntoTransport` and `rmcp::RoleClient` in its signature, and `revision.rs`
   exposes `modern_revision`, `sdk_default_revision`, and `describe_negotiated`, all returning or taking
   `ProtocolVersion`. So the doc sentence was not merely violated by `P3-009b`; it had been **wrong for two
   phases**, and every reader after `P3-008e` was misled in the reassuring direction — a reader checking "does
   an SDK type cross this boundary?" would have concluded no.
3. **`P3-009b` added a further leak with no comparable justification.** `connect_over` is a documented test seam
   and `revision.rs` exists to make the `LATEST = V_2025_11_25` trap executable; a public `sdk()`/`service()`
   pair was neither.

## Decision

**1. The boundary is enforced by a test, and a new leak fails it.**

`boundary_tests.rs` reads every `.rs` file under `src/`, drops `#[cfg(test)]` regions, and reports each public
declaration whose signature names the SDK — by **fully-qualified path or imported name**. Both forms are
checked, because the first version's omission of the second is what let the violation through.

**2. The genuinely justified exposures are recorded as an exception list, with reasons.**

| Declaration | Why it is public |
| --- | --- |
| `connect_over` | a test seam: the negotiation tests drive the real SDK over an in-process duplex pair, and `P3-007` recorded that a fixture sharing the code's assumptions cannot find a revision defect |
| `modern_revision` | the SDK's constant for the target revision, asserted against the JARVIS-owned string `MODERN_REVISION` so a pinned-SDK change is caught rather than absorbed |
| `sdk_default_revision` | observational only, so a test can assert the SDK's default is **not** the modern revision — the trap as an executable fact |
| `describe_negotiated` | takes the SDK's type **deliberately**, so a caller cannot pass a bare string that disagrees with what was negotiated |

A companion test asserts **each entry still matches a real declaration**, so the list cannot rot into an
endorsement of anything: a rename or a removal fails, which makes deleting an exception a change a reader must
make deliberately.

**3. `P3-009b`'s own leak was removed rather than recorded.**

- `sdk()` and `service()` are **private**.
- `JarvisMcpServer::tool_list` and `invoke` are **`pub(crate)`**, because their return types are the SDK's
  `Tool` and `CallToolResult`. The trait impl is what a client reaches, and `serves()` — which returns a `bool`
  — is the public statement of the same rule.
- `SERVED_PROTOCOL_VERSION` is a private `const`, and **`served_protocol_version()` returns the wire string**.
  The `&'static str` is what a caller outside this crate needs: it is what a client sends and what this server
  compares against, so the SDK's newtype adds a dependency at the boundary and nothing else.
- `call_result` is `pub(crate)`; `bounded_reason` and `method_not_found_code`, which `P3-009b` exported and
  nothing called, were **deleted**. `adapter.rs` already had its own `bounded_reason`, so the exported one was
  dead code as well as an SDK-typed leak.

**4. The one not-yet-consumed configuration carries `#[cfg_attr(not(test), expect(dead_code, ..))]`.**

`sdk` and `service` are reached only from the transport tests, because **this slice does not bind**. An
`expect` fails once the lint stops firing, so the binding slice must delete the attribute; an `allow` would sit
there indefinitely. The expectation is scoped to the non-test build because that is the only configuration where
it holds — an unconditional `expect` is unfulfilled in a test build, which is the same "name the configuration
a claim is true in" discipline the rest of this phase has needed.

**5. The documentation sentence is corrected rather than left as aspiration.**

`repository-layout.md` now names the four exceptions and points at the test. A claim that is false in a doc is
worse than an exception recorded in code, because the doc is what a reader trusts.

## Consequences

- **The invariant now has a mechanism.** A new SDK-typed public declaration fails with its file, line, and text;
  a stale exception fails too. The falsification that drove this is preserved: the scan's own unit tests include
  a public declaration naming the SDK in both forms, and a private one that must **not** be reported.
- **Three dead items were removed by the same review** (`bounded_reason`, `method_not_found_code`,
  `SdkSurfacePosture`, and a `ServedEndpoint` wrapper kept only to reach an SDK type publicly). The wrapper is
  worth naming: it existed **because** the public `service()` needed an SDK-free return, so removing the real
  leak removed its reason to exist.
- **The generalisable lesson is about how a limit gets written.** "No SDK type appears in our public surface"
  was recorded as a *fact* about the codebase when it was an *intention*, and it survived two phases because
  nothing could contradict it. The rule this phase has now applied five times: a claim in a doc, a test comment,
  or a commit message is read downstream as evidence, so it needs the same verification as a vendor's.
- Still open, recorded rather than implied: `connect_over`'s visibility should move behind an off-by-default
  feature so the **shipped** surface is SDK-free, and that is a follow-up rather than half-built here. **Nothing
  is bound**, so `ServingConfig` remains reachable from a test and not from a client (`P3-009c`).

## Alternatives rejected

- **Leave the sentence as it was, with exceptions in code only.** A doc that says "none" while four exist is
  worse than one that says "four, here is why" — the first is a false claim a reader acts on.
- **Enforce the invariant by deleting every exception.** `connect_over` would lose the negotiation tests that
  `P3-007` proved are the only way to find a revision defect, and `revision.rs`'s helpers would lose their
  reason to be executable.
- **Make `tool_list`/`invoke` public and accept the leak.** They exist for a test, and the test is in-crate, so
  visibility costs nothing.
- **Return `ProtocolVersion` from a public `served_protocol_version`.** The wire string is the useful form and
  the SDK's newtype is not JARVIS vocabulary.
- **`#[allow(dead_code)]` instead of `expect`.** An `allow` has no expiry, so the binding slice could leave it
  and a reader would conclude the item is still unbound.
- **An unconditional `#[expect(dead_code)]`.** Unfulfilled in a test build, so it fails the lint gate — the claim
  is only true in one configuration, and the attribute must say which.
