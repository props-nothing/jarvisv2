# ADR-0026: Discovery is joined to authority only where both halves are visible

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-008a` through `P3-008e` built two things that did not touch each other. `jarvis-mcp` could
translate a tool listing nobody had fetched, and `jarvis-mcp-transport` could fetch a tool listing
nobody had translated. Every one of those five slices recorded the same honest limit: **a capability a
caller can use, not one an operator can reach**, and "nothing calls any of this".

That is not a coincidence and it is not only a scheduling artefact. It is a structural consequence of
splitting authority from transport, which is a decision this project made deliberately and would make
again: `AGENTS.md` requires that provider SDK types not cross a JARVIS boundary, and `P3-007` found a
real defect in the SDK's lifecycle handling that would have been invisible inside a crate that had
already coupled to the SDK's data model.

So the two halves are correct and neither alone can be tested for the thing that matters — **whether a
listing that actually arrived over a wire becomes a definition with the right posture.** The
`ServerListing` inputs a catalog takes can be constructed by hand, so a catalog test proves the catalog
and says nothing about the join. And the join is where defects have been found in this project with
monotonous regularity: `P3-006a` found that two correct modules left a receipt able to understate the
risk it was issued for; `P3-006b` found that a request could carry a receipt authorizing a *different*
tool; `P2-009b` found that the context assembler's correct ordering produced the wrong conversation.
Each was a defect in the space between two correct components.

## Decision

**1. The join lives in `jarvis-mcp-transport`, and it is a function over connections.**

`build_catalog` takes `HostedServer { configured, connection }` values and returns a `HostBuild`. It
does not open connections: whether a server is a child process or an HTTP endpoint is an operator's
configuration question and a daemon concern (`P3-009`), so accepting established connections keeps the
join testable over an in-process pair while the spawn path has its own tests.

**2. The operator's name and posture travel with the connection, in one struct.**

`ConfiguredServer` already holds the name, and `HostedServer` holds both halves, so a policy paired
with the wrong connection is not expressible by a caller who passes one value. Two values that must
agree, with nothing holding both, is the `P3-006a` defect class; the fix there was to make the
disagreement unrepresentable rather than checked, and the same move applies here.

**3. The reported identity is read from the connection, never from the listing result.**

This is the field the whole drift check is built on. `P3-008d` compares a server's `ReportedIdentity`
between builds to notice that the process behind an operator's chosen name changed. If the host
*defaulted* that value, or read it from the tool-list result instead of the negotiated peer info, the
comparison would be a value against itself and the check would never fire — a guard that cannot fail,
which is worse than no guard because it reads as coverage. A test asserts the two fields stay
different across a real negotiation.

**4. A server that cannot be read does not fail the build, and the reason is preserved separately.**

One flaky third-party process must not empty the model's tool list. The catalog already reports a
server that contributed nothing as "the server offered no tool listing", which is true whether the
server was unreachable, refused the request, or genuinely had none — and the remedies differ. So
`HostBuild::unreadable` carries a `reason` per server that distinguishes them, while the catalog keeps
its own less specific exclusion. Both are kept because they answer different questions: which tools a
model will not be offered, and why a server is missing.

**5. A server that declared no `tools` capability is not asked.**

"Does not do tools" and "has no tools" are different facts, and only the first is knowable before the
request. Asking anyway would work — the specification permits an empty list — but it would collapse the
two into one observation. A test asserts the request was *not* sent, so the distinction is enforced
rather than intended.

**6. `tools/call` uses the single-round request, and the modes JARVIS cannot serve are refused by
name.**

`2026-07-28` added MRTR (a server may answer with `input_required` and expect the client to retry with
answers) and moved Tasks to an extension. The SDK offers a helper that drives MRTR rounds by invoking a
client `ClientHandler`; this crate registers none, because the only human in JARVIS answers through
JARVIS's own approval path rather than a third-party server's form. Using the single-round request
makes both modes arrive as **named refusals** (`CallError::InputRequired`, `CallError::Task`) instead
of an SDK error about a missing handler — or, worse, an empty success for a call that had not finished.

**7. A tool that reports failure is a result, not an error.**

The protocol carries two kinds of bad news and they belong in different layers. A JSON-RPC error means
the request was refused and nothing happened, so it is a `CallError`. A `CallToolResult` with `isError`
set means the tool *ran* and could not do what was asked — an outcome. Folding the second into the
first would lose the distinction `ToolOutcome` exists to make and would make a tool's honest refusal
look like a broken connection.

**8. A decode failure is its own error variant, not a transport fault.**

The protocol's result union is deserialized as an **untagged** enum (measured in the pinned SDK's
`model.rs`: `ts_union!` emits `#[serde(untagged)]`). A result whose declared `resultType` disagrees
with its fields therefore does not fail as "a malformed `input_required`" — it becomes the SDK's
generic `UnexpectedResponse`, which names nothing. Classifying that as "the call did not complete"
describes an unreachable peer and sends an operator to inspect a working connection, so it has its own
variant (`CallError::Undecodable`) whose text names the actual cause. This was found by writing the
task fixture **wrongly** (nesting the task instead of flattening it, which `CreateTaskResult`'s custom
deserializer requires) and reading the message a user would have seen.

## Consequences

- The recorded limit "a capability a caller can use, not one an operator can reach" is **narrowed, not
  closed**: nothing still reads a `config.toml` or constructs a `HostedServer` outside a test, so the
  daemon (`P3-009`) is still what makes MCP servers reachable. What changes is that the join is now a
  tested function rather than an unwritten step, and `P3-009` has one composition to call instead of
  two to reconcile incorrectly.
- `connect_stdio` is now exercised against a real child process, which `P3-008e` recorded as having
  **no test at all**. The fixture is a hand-written server (`src/bin/fixture_peer.rs`) behind the
  off-by-default `fixture-peer` feature, so it cannot be mistaken for a shipped binary, and it writes
  the wire framing itself rather than using the SDK — a client and server built from one library share
  their assumptions, so a disagreement with the specification would pass.
- `connect_http` remains **unexercised**, and this is recorded rather than glossed: it needs a live
  endpoint or a scripted HTTP server, and the Streamable-HTTP transport's own behaviours (Origin
  validation, `MCP-Protocol-Version` headers, SSE framing) belong to a slice that can stand up a real
  peer. The honest position is that stdio is proven and HTTP is not.
- A dead `ListError::NoIdentity` variant was **removed during the review that preceded this slice**. It
  was declared, described as "a signal worth surfacing", and constructed by nothing — absence of an
  identity is an empty `ReportedIdentity` on a working value, which is what makes a server that *stops*
  naming itself observable. A declared-but-unconstructed error variant reads downstream as a live
  condition, which is the `expected_version` defect class corrected at `P3-006c`.

## Alternatives rejected

- **Put the join in `jarvis-mcp`.** It needs `McpConnection`, which holds an SDK type, so this would
  either give the pure crate the SDK dependency or invert the crate split. `repository-layout.md` places
  composition in the crate that can see both halves.
- **Let the daemon call `connect_*` and `McpCatalog::build` separately.** That is the composition as it
  stands today, and it is the shape that produced five slices of "nothing calls this". Two calls mean
  the identity and the tools can be read from different sources, which is exactly the defect item 3
  guards.
- **Fail the whole build when one server is unreadable.** Would make a single flaky third-party process
  an outage, and one third-party process is the least reliable component in the system.
- **Use the SDK's MRTR-driving helper and register a stub `ClientHandler`.** Declaring a capability
  JARVIS would answer with a stub tells the server to depend on behaviour that does not exist — the
  rule already applied to `sampling`/`roots`/`elicitation` in `client_config`.
- **Treat a task answer as success with no content.** The server materialized background work; a call
  that reported success would tell the caller the work finished when it had not started.
