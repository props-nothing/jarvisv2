# ADR-0030: The daemon dispatches by tool identity, and MCP servers live in their own document

**Status:** Accepted

**Date:** 2026-09-23

## Context

Ten consecutive MCP slices (`P3-008a`..`P3-008i`) recorded the same honest limit in ten different
wordings: **"a capability a caller can use, not one an operator can reach"**. The pure crate could
translate a listing nobody had fetched; the transport could fetch one nobody had translated; the adapter
could run a call nothing dispatched; the host role could parse a configuration nobody read. Every piece
had tests, and a grep for either MCP crate in `apps/jarvisd` returned **zero** hits. `P3-008i` closed by
saying so in its own entry: "still not reachable from `jarvisd` ... `P3-009` is the one call that loads
the section and holds a host."

This slice is that call. It is also the first place where **three previously separate things are visible
at once** — the registry's tool list, the MCP catalog's routing, and the filesystem adapter's grant —
which in this project is where defects have been found with monotonous regularity (`P3-006a`, `P3-006b`,
`P2-009b`, `P3-008f`, `P3-008i`).

One finding from the slice's own construction decided its shape. `tool_pipeline_tests.rs` gained a routing
test — "a call reaches the adapter that owns its definition" — and falsifying it by replacing the lookup
with `self.adapters.values().next()` left the suite **green**. The test had registered only one adapter, so
"return any adapter" found the correct one. The test proved the dispatch table was non-empty and nothing
whatsoever about routing.

## Decision

**1. The dispatch table is its own type, keyed by canonical identifier, and it refuses two things rather
than resolving them.**

`Dispatch` holds `BTreeMap<String, Arc<dyn ToolExecutor>>`, built from `(definitions, adapter)` pairs and
never from a caller's list of identifiers — a caller-supplied list would be a second statement that can
disagree with the adapter it describes.

- **`DispatchError::Uncovered`** — a tool the registry offers that no adapter can run. Checked at startup
  against the registry's own inventory, because the alternative is a call that passes schema validation,
  passes policy, **writes its durable admission row**, and *then* fails with nothing able to run it. The
  failure would surface after the point of no return.
- **`DispatchError::Duplicate`** — two adapters claiming one identifier. Refused rather than resolved,
  because a winner chosen by registration order means the same call reaches different providers on
  different builds.

It is keyed by identifier because that is the key the **authorization receipt** binds
(ADR-0021). Keying by adapter would mean scanning, and a scan returns the first match — an implicit
ordering decision, which is the one thing this type exists to make impossible. `adapter_for` returns
`Option`, so a lookup cannot default to another adapter; `McpCatalog::route` already refuses to default one
level down.

**2. The MCP servers live in `mcp-servers.toml`, not in an `[mcp]` section of `config.toml`.**

Two reasons, and the second decided it:

1. The daemon's configuration schema is validated by **key allowlist**. An `[mcp]` section would require
   `jarvis-storage`'s schema to learn one protocol's configuration vocabulary — a dependency from a
   storage adapter into an MCP shape. `repository-layout.md` forbids that direction of arrow for core and
   application crates, and it is equally wrong for this one.
2. **An unusable MCP server must not stop the daemon.** A third-party program that is missing, a server
   that hangs, a collision between two configured servers — each is a reason for the *MCP tools* to be
   unavailable, not a reason for the daemon to refuse to serve anything. Separate documents mean the
   daemon's own startup path cannot be affected by anything on the other side of that boundary. This is
   the same reasoning that makes one failed `tools/list` an exclusion rather than a failed run.

So MCP composition failure is **logged, not fatal**, while pipeline failure **is fatal** — the pipeline
covers the filesystem grant and the registry, and starting anyway would serve a tool set nobody declared.

A missing `mcp-servers.toml` is **not** an error: a daemon nobody configured servers on is the normal
case. A file that **exists and cannot be read** is an error, because an operator who wrote one and has it
unreadable must not conclude their servers were configured and empty.

**3. A collision refuses the MCP host, and that is a refusal to serve one thing rather than a refusal to
start.**

Serving a catalog whose contents depended on configuration order would make the reachable tool set a
function of an ordering nobody declared meaningful — the reasoning `McpCatalog` already applies one level
down. The daemon logs the failure and offers no MCP tools.

**4. The pipeline takes `Option<WorkspaceRoots>`, so an absent grant means an absent adapter.**

`WorkspaceRoots::new` **refuses an empty list** (`RootError::NoRoots`), deliberately: a tool must not
silently read nothing while looking like a tool that works. A daemon with MCP servers and no filesystem
grant is therefore `None` here, and the filesystem adapter is **not registered at all** — the honest shape,
where the tool is absent rather than present-and-failing, and the same reasoning that already made a
zero-root daemon compose no pipeline.

This was found the right way: the routing test failed with `NoRoots`, and the failure was the composition
being wrong rather than the test.

**5. The actor grants both scopes; the narrow constructor stays.**

`ToolActor::workspace_and_mcp` grants `files.read` **and** `mcp.call`, and the gateway uses it.
`workspace_reader` grants only `files.read`, so **every MCP call would have been denied for a missing
scope** — a denial that would read as a policy problem rather than as the actor being under-granted.

Widening `workspace_reader` was rejected. The two grants are genuinely different capabilities and the
daemon now serves either or both: a profile with no filesystem roots has no filesystem tool to read, so
granting `files.read` there is a grant with no consumer — the thing this type's narrowness exists to
avoid. Both literals are parsed **before either is used**, so a malformed constant yields `None` rather
than a partial grant; failing closed to a *subset* would produce a denial that looks like policy.

A test asserts `MCP_CALL_SCOPE` equals `jarvis_mcp_transport::DEFAULT_MCP_CALL_SCOPE`, so the two copies
of that literal cannot drift apart without a test failing.

**6. A routing test must have at least two candidate adapters, and it must assert that precondition.**

The falsification that passed (`values().next()` finding the right adapter because there was only one)
found a test that could not fail. The test now registers the filesystem adapter beside the MCP one and
asserts `dispatchable_tools() == 3`, so the precondition is stated in the test rather than assumed from the
fixture. `dispatchable_tools` is `#[cfg(test)]`: nothing in the product asks for a count, and adding a
public accessor nothing calls is how a surface grows a method with no consumer.

**7. The host is composed once, and its adapters are cloned out of it rather than derived later.**

`ComposedHost` holds the live `McpHost` **beside** the adapters. The adapters share the host's connections
through their own `Arc<McpConnection>`, which is why the host can still close them at shutdown — a
connection is closed by its last owner. Deriving the adapters at registration time instead would mean the
host had to be borrowed across the pipeline's construction, and the pipeline owns its adapters for the
process's life.

Each adapter is built from `adapters_with_definitions()`, so an adapter's definitions and the registry's
come from **one** source: the transport, because it is the only place that knows which server owns a
canonical identifier. Deriving them separately here would let them disagree about which tools exist.

## Consequences

- **The limit ten slices recorded is closed.** `apps/jarvisd` reads a document, connects a host, registers
  its adapters, and dispatches an authorized request to the adapter the identifier names. `has_cross_server_collision`
  acquired a caller in `P3-008i` and now acquires an **operator-visible effect**.
- **`Dispatch` is tested against the failure it exists to prevent.** `adapter_for` changed to
  `values().next()` now fails with `NotImplemented { tool: "mcp.test.search" }` — the misroute as evidence,
  rather than a passing run that proved nothing.
- **Four previously recorded limits in `TODO.md` became false and were corrected in place**, each with a
  `CORRECTION (2026-09-23)` naming the slice that closed it (`P3-008e`, `P3-008f`, `P3-008g`, `P3-008h`,
  `P3-008i`). They all read "`apps/jarvisd` has zero references to either MCP crate" — the pattern recorded
  in memory, where a claim written from intent propagates as evidence because nothing distinguishes a claim
  that was checked from one that was assumed.
- **The daemon's failure policy is now split by owner, not by severity.** MCP: log and continue. Registry
  and filesystem grant: refuse to start. That asymmetry is the decision, and it follows from which
  documents own which keys.
- Still open and recorded rather than implied: no `run_events` row for an MCP call (`P3-012`); no pooling
  or reconnect, so a server that dies stays unavailable until restart; `seen_before` is always empty, so an
  identity drift across a restart is invisible; `NamingStrategy` is hardcoded to `Prefixed` and `Bare` is
  unreachable from configuration; a collision's affected servers are named only in a log line.

## Alternatives rejected

- **Register identifiers from a caller's list and look up by scanning.** A scan returns the first match,
  which is an ordering decision nobody declared — and it would make the duplicate case unresolvable rather
  than refused.
- **Let a duplicate claim be resolved by registration order.** The same call would reach different
  providers on different builds.
- **Put the MCP servers in an `[mcp]` section of `config.toml`.** One schema learning a protocol's
  vocabulary, and an MCP failure becomes a daemon startup failure.
- **Treat a missing `mcp-servers.toml` as an error.** Every existing profile would fail to start.
- **Build an empty `WorkspaceRoots` for the MCP-only case.** Contradicts `RootError::NoRoots`, which exists
  so a tool cannot silently read nothing — and the contradiction surfaced as a failing test.
- **Widen `workspace_reader` to grant `mcp.call`.** A grant with no consumer in a profile that has no MCP
  servers, in the type whose whole reason for narrow constructors is that a grant is what a bug widens.
- **Hold the adapters only in the host and look them up there.** The pipeline owns its adapters for the
  process's life, so the host would have to be borrowed across the pipeline's construction.
