# ADR-0119: The model may request a tool; deterministic policy still decides whether it runs

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** the agent tool loop (the executor's bounded multi-step tool round trip). Closes the gap
  `P2-009` recorded — *"`MAX_MODEL_CALLS` is one because the executor does not plan or use tools"* — which
  is the condition the whole tool/policy/approval pipeline was built for and never exercised through the
  model.
- **Relates to:** `ADR-0005` (canonical tools and MCP), `ADR-0017` (policy evaluation outcomes), `ADR-0018`
  (an approval is bound to the exact arguments), `ADR-0019` (tool calls are a durable lifecycle), `ADR-0023` (the tool
  pipeline composition root), `ADR-0010` (turn detection is model-based), `ADR-0014` (a conversation is runs
  in a session).

## Context

`AGENTS.md` states the platform's central rule: *"The model may request an effect; deterministic Rust policy
decides whether it may happen. All tool calls pass through schema validation, authentication, authorization,
risk classification, approval policy, timeout, idempotency where relevant, execution, and audit."*

Every one of those seven gates was built and tested by Phase 3, and the run state machine in
`docs/architecture/runtime-and-models.md` documents `Planning → Executing → Observing → Planning` as the tool
round trip. But the executor that drives a run did **not** offer the model any tools:

- `MAX_MODEL_CALLS` was `1`, with the comment *"One, because `P2-009` does not plan or use tools"*.
- `ChatRequest` had no `tools` field at all, so `ToolRegistry::discover()` — the model-facing tool surface —
  had **no consumer**.
- The OpenAI adapter deliberately did not decode streaming `tool_calls` deltas, so a provider's tool call
  could not reach the domain layer even if a model emitted one.

The consequence was that the platform could *serve* tools (over HTTP and as an MCP server) and could *run* a
tool a caller named, but the model could not ask for one. A user asking JARVIS to read a file, search mail,
or check a calendar received an answer generated without any of it. The pipeline was a capability a caller
could use and not one the agent could reach.

## Decision

**1. The model is offered exactly the tools the pipeline can run, from one source.**

The offered set is `ToolRegistry::discover()` — the same registry the dispatch table was verified against, so
a tool offered to a model is a tool the pipeline can resolve. Deriving the list any other way would be a
second statement of which tools exist, and the two could disagree in the direction that matters: a tool
offered but unrunnable. The **input schema** is projected from the definition the pipeline validates against,
so the schema the model reads is the one its arguments are checked with. Effects, risk, scopes, and the
approval policy are deliberately **not** offered: a model chooses *what* to call, and policy decides *whether
it may*, so exposing policy fields would invite the model to reason about its own authority.

**2. A run is a bounded agent loop, and the bound counts effects.**

`Planning → Executing → Observing → Planning` repeats while the model requests tools, and
`Observing → Responding` ends it when the model answers. Two independent bounds apply: `MAX_MODEL_CALLS`
bounds provider calls, and `MAX_TOOL_CALLS` bounds executions — separately, because one model call may
request several tools, so a call bound alone would not bound the number of effects a runaway loop can
produce. Exceeding either fails the run with an explicit code rather than looping.

**3. A tool call re-enters the ordinary pipeline; there is no "agent tool" path.**

Every invocation goes through `ToolPipeline::call_tool` — schema validation, policy evaluation, durable
admission, approval policy, idempotency ledger, adapter execution, and audit. The executor adds a *loop*; it
adds **no authority**. The actor is built from the **stored run** (its workspace and identifier), never from
the model, because a model that could name its own workspace would widen its own authority. This is the same
rule the HTTP tool route already follows, applied on the in-process path.

**4. A refusal or a fault is fed back to the model; it does not fail the run.**

A policy denial, a schema violation, an unknown tool, or an adapter fault becomes the **tool result** the
model reads, with its reason code where there is one. The model can then answer truthfully or try something
else. Failing the run instead would end a conversation over one denied or mistyped call, and the model would
never learn the tool was unavailable. A **missing tool surface** (a model requests a tool when the daemon
composed no pipeline) is the exception: that is a configuration fault and fails the run, because an answer
that claims to have used a tool nothing ran is the failure the system prompt warns against.

**5. A held call parks the run at `AwaitingApproval`.**

An invocation policy holds for human approval moves the run to `AwaitingApproval` and stops. Nothing is fed
back to the model, because the effect has not happened, and the assistant turn that requested the call is
already in the transcript. The decision and resume arrive through the **approval and resume routes**
(`P3-017`/`P3-018`), which complete the call directly; the executor parks a run truthfully and does not hold a
promise those routes already keep. `AwaitingApproval` is reachable in the loop only after a restart while the
run waited, in which case the run is left parked rather than advanced on a decision nothing has taken.

**6. A tool result is replayed as a `tool` message correlated to the assistant turn that asked.**

The assistant message carries the tool calls it requested, because a provider rejects a `tool` result whose
originating assistant turn is absent. `ChatMessage::is_consistent` enforces the three rules that keep a
replayed turn a shape a provider can accept: a tool result names its call, only a tool result carries that
identifier, and only an assistant turn carries tool calls.

**7. Streaming tool calls are reassembled before they mean anything.**

A provider delivers one invocation across several chunks. The adapter accumulates fragments by the provider's
`index` and emits **one** `StreamEvent::ToolCall` per invocation, at the point the turn finishes — never a
fragment, and never an invocation missing its identifier or name. This preserves the property the previous
code enforced by refusing to decode tool calls at all ("a partially assembled invocation must never be
reported as a complete one") while making a complete one reachable.

## Consequences

- **The pipeline has its model-facing consumer.** `ToolRegistry::discover`, the policy engine, the approvals,
  and the tool lifecycle are now reachable by the model, which is what they were built for.
- **A run's stream explains the tool round trip.** `tool_requested`, the state changes, and the final
  `output_completed` are durable, so a client replaying the stream sees the run ask, run, interpret, and
  answer rather than jump from planning to an answer.
- **Usage is recorded every model call,** not only the last, because an agent loop is several billable calls.
- **The tool surface is re-derived each call,** so it is always the current registry rather than a copy that
  could go stale.
- **A `tools` field and tool-call reassembly live in `jarvis-models`,** keeping the provider-neutral contract
  complete: `ToolSpec` is projected from a canonical `ToolDefinition` at the composition root, so the model
  crate never depends on `jarvis-tools`.

## Alternatives

- **Keep the single-call executor and expose tools only to MCP/HTTP callers.** Rejected: it is the state this
  slice closes, and it makes the model a narrator rather than an agent.
- **Pass tool results back as a plain user message.** Rejected: a provider rejects a `tool` result without the
  assistant turn that requested it, so the transcript must carry both.
- **Let a held call block the loop awaiting the decision in-process.** Rejected: the decision arrives through a
  different route (a client's `POST /approvals/{id}/decision` and `POST /calls/{id}/resume`), and holding the
  executor open across a human decision would make a restart lose the run. Parking at `AwaitingApproval` is
  the durable, restart-safe shape the state machine already defines.
- **Fail the run on any tool refusal.** Rejected: a denied or mistyped call is a normal event the model should
  be able to recover from; failing would make one bad tool name end a conversation.

## Conditions that would justify revisiting

- A provider whose streaming tool calls cannot be reassembled by index (a different fragment key), which would
  require the accumulator to learn the provider's shape.
- A measured need for the executor to await a decision in-process (for example a same-turn approval on a
  trusted local channel), which would change decision 5 and needs its own ADR because it moves a trust
  boundary.
- A tool surface large enough that offering every registered tool on every call exceeds the provider's request
  bound, which would require a selection or pagination rule for the offered set.
