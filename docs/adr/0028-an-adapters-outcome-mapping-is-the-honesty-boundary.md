# ADR-0028: An adapter's outcome mapping is the honesty boundary, and every row of it needs a test

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-008f`/`P3-008g` got a tool call to work over the wire and recorded the same limit in each entry:
**"nothing drives it from a run"**. `tasks/call` worked and nothing in the product asked for one. Making
an MCP server's tool a real `ToolExecutor` closes that, and it turns out to be mostly a *conversion*
problem rather than a transport one — the transport is already proven, and what is new is deciding what a
server's answer **means**.

That conversion is where an MCP call becomes either a record or a lie. `jarvis_tools` already draws the
line precisely, in two places:

- `AdapterError::RefusedBeforeReaching` says **nothing happened**, which is safe to retry.
- `AdapterError::AmbiguousAfterReaching` says **the provider was reached and the outcome is unknown**,
  whose own doc notes the reason the variant exists: "a caller that saw a plain error would retry — and
  for a non-idempotent effect the retry is a second effect."
- `ToolOutcome::{Confirmed, Failed, Unknown}` are the established states, with `Confirmed` requiring
  evidence and `Failed` requiring a reason.

So every row of the mapping decides whether an effect may be repeated. Getting one wrong in the
permissive direction is how one sent message becomes two.

## Decision

**1. The mapping is stated as a table in the module, and each row's reasoning is written down.**

| Server said | Adapter reports | Why |
| --- | --- | --- |
| a result, `isError` false | `Confirmed` + evidence + output | The provider answered with a result. The evidence is a **locator**, not the content. |
| a result, `isError` true | `Failed` + reason | The tool ran and refused: nothing happened, and the reason is known. |
| a JSON-RPC error | `ProviderRefused` | The peer answered and refused the request, so nothing happened. |
| a transport failure after sending | `AmbiguousAfterReaching` | The request may have been received. The safe direction. |
| an undecodable answer | `AmbiguousAfterReaching` | The server answered, so it ran something, and the outcome cannot be established. |
| `input_required` / `task` | `ProviderRefused` | Modes JARVIS declared no capability for; the server reached its own boundary. |
| an unrouted identifier | `NotImplemented` | A registry mistake, not a provider failure — different remedy. |
| past its deadline | `RefusedBeforeReaching` | Nothing left this process. |

**2. Evidence is a locator and output is content, kept separate.**

`mcp:<operator's server>/<server's tool name>` is the evidence; the server's text or structured content is
the output. `tools-and-connectors.md` requires "provider IDs preserved separately from user-facing text",
and the split is asserted rather than assumed — a reader must be able to tell which is which. The
operator's name is used, never the server's own claim, because ADR-0024 makes that claim evidence and not
an identifier; using it would put two names for one server into the audit trail.

**3. A contentless success is `Confirmed` with **no** output, not an empty output.**

An empty string is a *value* a tool might legitimately return, so recording one would make "the tool
returned nothing" indistinguishable from "the tool returned an empty string". The call is the evidence.

**4. Routing is held by the adapter, never carried in the arguments.**

The first implementation put the server's own tool name into the request's `arguments` under a reserved
key. **That was wrong twice, and both reasons are already load-bearing elsewhere in this project:** the
arguments are validated against the tool's input schema, so an injected key is a schema violation; and
they are covered by the authorization digest, which `ToolExecutionRequest::new` **recomputes and
compares**, so injecting a key after the receipt was built makes every request fail its own binding check.
Both properties exist deliberately — one is the `P3-001` contract, the other is `P3-006b`'s binding — so
the routing moved into the adapter, supplied from the catalog entries at construction. A lookup at call
time would be worse still: a catalog refresh between authorization and execution could change which tool
runs, while the receipt binds the *identifier*.

**5. The routing is filtered to one server at construction.**

An entry belonging to another server is skipped, so server A's identifier cannot be sent to server B under
a name B happens to recognize — which would be a call executing with the wrong server's declared posture.

**6. A reason is bounded to fit the stored limit **by construction**.**

`ToolOutcomeRecord::failed` refuses a reason longer than `MAX_OUTCOME_DETAIL_CHARS`, so a reason built
here must fit or the `Failed` silently becomes `Unknown`. The prefix counts toward the budget, and a tool
name so long that the prefix alone would fill it gets a reason with no tool in it rather than a truncated
prefix — truncating the prefix would hide which tool the reason is about, which is the part a reader
needs. A test asserts a maximal reason is *recorded*, not merely short.

## Consequences

- **The limit every MCP slice recorded is now closed at its last step**: a policy-authorized
  `ToolExecutionRequest` reaches a real server and produces a real `ToolCallResult`. 15 adapter tests drive
  the conversion through a real connection, with the receipt **derived** from a real `PolicyDecision` over
  the same arguments — so a fixture cannot describe a tool the receipt does not cover.
- **A falsification found a missing test, and that is the most useful outcome of this slice.** Changing the
  `Unavailable` mapping from `AmbiguousAfterReaching` to `RefusedBeforeReaching` left the whole suite
  **green**: no test covered a transport failure at all, so the one row that decides whether a
  non-idempotent effect may be repeated was unpinned. The scripted peer gained a `HangUp` reply — a method
  answered with silence and a **closed** connection, which is the only way to express "sent, no answer" —
  and the test now fails the falsification with the exact wrong claim ("the adapter refused the call
  before reaching a provider: Transport closed"). **A row in a mapping table is a claim until a test
  pins it.**
- Two fixture mistakes were also instructive and are recorded in the test: an MCP tool requires the
  `mcp.call` scope (an empty grant is a `MissingScope` denial), and **even a minimal-risk tool requires
  `ChannelEvidence`**, so an `Absent` strength claim is an `InsufficientAuthentication` hold. Both turned
  every test into a held call, and both were visible only because the fixture asserts an allowance rather
  than assuming one.
- Still **not reachable from `jarvisd`**: nothing composes an `McpToolAdapter` into the pipeline or reads
  a server from `config.toml`, so `P3-009` remains the gate for operator reachability. Nothing writes a
  `run_events` row for an MCP call either — `P3-012` owns linking calls to the event log.

## Alternatives rejected

- **Return `Failed` for a transport failure.** A retry would then be safe by the vocabulary and would
  duplicate an effect that may already have happened — the exact defect `AmbiguousAfterReaching` exists to
  prevent, and the reason its doc says an adapter "must only make that statement when it is certain".
- **Report `Confirmed` with the content as evidence.** Conflates a locator with a result, which is the
  split `tools-and-connectors.md` requires, and makes a stored confirmation uncheckable.
- **Put the routing in the arguments.** Rejected above; it violates the input schema and the authorization
  digest simultaneously.
- **Resolve the routing by looking it up during the call.** Lets a catalog refresh between authorization
  and execution change which tool runs.
- **Let `unrecordable` fabricate an outcome.** A fallback that invents a state would be worse than the
  `Unknown` it replaces; `Unknown` is the honest answer for "the call happened and we cannot describe it".
