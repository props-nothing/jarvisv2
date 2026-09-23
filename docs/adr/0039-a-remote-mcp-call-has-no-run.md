# ADR-0039: A remote MCP call has no run, and its enforcement is the same enforcement

**Status:** Accepted

**Date:** 2026-09-24

## Context

Nine `P3-009` slices built the pieces of an inbound MCP endpoint and each recorded the same limit in a different
form: *"nothing binds, so no request reaches this gate."* `P3-009c-a` built the layer a listener mounts and still
recorded that **the daemon does not mount it**. At that point every part existed — a served surface, a handler, an
enforcement gate, a binding layer, a caller allowlist — and none of them had ever been reached by a request,
because no composition root had wired them together.

Mounting it raises a question the earlier slices deliberately left open. Every other tool path in this daemon
attributes a call to a **run**: `ToolPipeline::call_tool` is handed a `CallOrigin` naming the run and the actor an
operator's invocation carries. An MCP request carries none of that. It has no run, no session, and no actor — it
has a method name, arguments, and whatever a client chose to send. So *whose* call is it, and what does the
pipeline enforce when it cannot say?

## Decision

**1. A remote MCP call has no run, and the schema is the authority for that rather than a comment.**

`0007_tool_calls.sql` declares `run_id TEXT NOT NULL REFERENCES agent_runs (id)`. A call with no run therefore
**cannot be written** as a `tool_calls` row: the foreign key refuses a null, and any non-null value would name an
agent run that does not exist. Inventing a run identifier to satisfy the column was the tempting shortcut, and the
schema rejects it — which is the useful form of the answer. The domain fact ("an MCP call is not part of an agent
run") is stated in the storage layer, where it cannot be edited away by whoever is in a hurry.

So `ToolPipeline::call_remote_tool` writes **no call row**, and that is recorded as a limit rather than presented
as completeness. A remote call is logged and is not durably attributable until `P3-012`.

**2. A second entrypoint rather than a flag on the first.**

`call_remote_tool` is a sibling of `call_tool`, not a `call_tool(… , record: bool)`. A boolean parameter is a value
a caller can set wrongly and a reader cannot see the consequence of: the audit behaviour of every call site would
depend on an argument whose name does not say what is lost. Two entrypoints make "may this call write a row" a
property of *which function was called*, which is visible at the call site and impossible to pass by accident.

**3. Parity is achieved by calling the same code, not by re-implementing it.**

Both entrypoints reach the **same** `Self::validate` and the **same** `evaluate` over the same definition, the same
`WorkspacePolicy`, and the same dispatcher. The differences are exactly two, and both are stated in the code rather
than left to inference:

- the **audit record** (no `tool_calls` row, per decision 1);
- the **approval outcome** (decision 4).

A second implementation of a check is where an enforcement step gets forgotten. The falsification that matters here
is that removing a check from the remote path must fail a test — and the same mutation on the local path must fail
a *different* one, or the two paths are not actually shared.

**4. An approval is refused, not bypassed, and there are two independent checks.**

A decision of `RequireApproval` means the call must be parked in `awaiting_approval` until a human releases it. This
path has no run to park it in and no approval record to bind to it, and **running the call would be the unsafe
direction** — an approval that is skipped is worse than one that cannot be obtained, because the operator's policy
said "ask me" and nothing asked. So a held decision is returned as a **refusal**, with the reason code
`mcp_call_cannot_hold_an_approval` rather than the workspace's own code, because reporting `require_approval` would
tell a client an approval is obtainable.

There is a **second, independent** check: `definition.approval() != ApprovalPolicy::Auto` is refused with the same
reason code. The two are not redundant. The engine's threshold is a **workspace** setting; `ApprovalPolicy` is a
declaration on the **tool**. A tool that declares an approval its workspace would not ask for must still not run
here, and a test that varies only the workspace could never observe the second check.

**That last claim is measured, not argued: mutating the second check to `false && …` left the entire suite green
at 13/13.** Nothing covered it. A test was added that registers a real `ToolDefinition` declaring
`ApprovalPolicy::Ask` at risk 0 — so the default workspace threshold of `Moderate` cannot hold the call, making the
declaration the only thing under test — alongside a **counting** adapter that must not be reached. The mutation now
fails exactly that one test.

**5. The remote actor's scopes are derived from the served surface, and are never the outbound scope.**

`ToolActor::remote` holds the **union** of the served definitions' own `required_scopes`. A union rather than an
intersection, because the actor must be able to run *any* tool the endpoint advertises; an intersection would
satisfy a tool whose requirements are a subset and refuse the rest, making the served surface a promise the actor
cannot keep.

**The first version of that constructor granted no scopes at all**, on the reasoning that the narrowest grant is
the safest. Its own test caught the error: `FilesystemReadTool` declares `required_scopes: ScopeSet::single("files.read")`
and the engine requires the actor to cover the **tool's** declared scopes, so an actor holding nothing was refused
every call with `missing_scope`. The endpoint would have been built, bound, and answered every request with a
refusal — which an operator would read as a policy misconfiguration rather than as a defect in this constructor.
Falsified by reverting to `ScopeSet::none()`: four tests fail.

It does **not** hold `mcp.call`. That literal is this daemon's grant to call **someone else's** server, so
attaching it to an inbound call would put the outbound grant on the inbound direction — a scope with the right name
and the wrong direction, which is worse than a missing one because it reads as intentional.

**6. The endpoint gets its own port, separate from the REST transport's.**

`mcp_serve_port` is a configuration field distinct from `http_enabled`. The two listeners have different trust
boundaries and different answers to "who may call": the REST transport is the CLI's channel to this daemon, and
this endpoint is a third-party client's. Sharing a listener would make one port's admission policy govern the
other's traffic, and sharing a *flag* would make enabling one enable the other. `Some(0)` and "a port with no
granted workspace roots" are refused at configuration time, because both would produce a daemon that reports itself
ready on a listener that cannot answer.

**7. The `Send` contract is pinned in the type the binding layer returns.**

`into_service` returns `impl Service<…, Future = ResponseFuture> + Clone + Send + Sync + 'static`, where
`ResponseFuture` is a pinned boxed future. An opaque `impl Service` says nothing about whether `Service::Future`
is `Send`, and `axum::Router::fallback_service` requires it — so the omission surfaces at the **mount site**, in an
error naming *its* generic parameter rather than this layer. Naming the future makes dropping `Send` fail here, and
a test asserts the contract by naming the alias.

## Consequences

- **The limit nine slices recorded is closed.** `jarvisd` mounts the endpoint on loopback when `mcp_serve_port` is
  configured, applies the same gates an operator's own call passes, and stops the listener in an order that keeps a
  request from being served against an adapter that is going away.
- **A remote call is not durably attributable.** It is logged and has no `tool_calls` row, because that row needs a
  run. `P3-012` owns the durable link, and until it lands a remote call is **observable but not attributable** — the
  same limit `P3-009i` recorded for `RequestAdmission`.
- **`spent_budget` remains a literal `false`.** This layer has no counter, so the rate limit the gate can apply
  never fires. It is passed as a literal rather than a parameter so the gap is visible at the call site.
- **The origin list is not configurable.** `build_endpoint` builds `ServerExposure::loopback_only()` through
  `ServingConfig::new` rather than reading a configured list. A configurable list would be a value that can make a
  startup either fail or serve something nobody chose; the caller allowlist is where a deployment expresses *who*
  may call, and its default is already safe.
- **An adapter's own error is propagated unchanged.** `map_pipeline_error` returns `AdapterCall(error)` as-is
  instead of rewriting it. The distinction preserved is `AdapterError`'s three-way split —
  `RefusedBeforeReaching` (nothing happened), `ProviderRefused` (the provider said no), and
  `AmbiguousAfterReaching` (a request left and the outcome is unknown). Collapsing all three into "nothing was
  reached" would tell a client to retry a call whose effect is unknown, and for a non-idempotent tool the retry is
  **a second effect** — the exact failure that variant exists to prevent. Every other pipeline error is raised
  before the dispatcher is consulted, so mapping those to `RefusedBeforeReaching` is a statement the code supports.
  Tested by **constructing** the ambiguous value, because no adapter serving a tool in this daemon returns one.
- **Five mutations were run, one property each**, and each failed exactly the test that names it: the scope union
  (4 tests), the held-decision refusal (1), the tool-declaration refusal (1), the empty-surface refusal (1), and the
  caller policy (1). The last two are the ones that were **green before this slice's tests existed** — the
  measurements are in `docs/research/integrations/mcp.md`.
