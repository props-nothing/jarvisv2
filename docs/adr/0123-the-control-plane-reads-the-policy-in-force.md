# ADR-0123: The control plane reads the policy in force, and a preview is a decision not a prediction

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** the tool control-plane surface — `GET /tools` and `POST /tools/{tool}/preview`, plus the
  `jarvis tools` CLI group, so an operator can see what this daemon will do rather than what the document
  says it should.
- **Relates to:** `ADR-0122` (an approval override can only tighten, and the risk vocabulary moved to core),
  `ADR-0023` (the tool pipeline is composed in the daemon), `ADR-0011` (run events are durable; HTTP is a
  first-class transport), `ADR-0037` (an admitted request is a type), `ADR-0022` (cancellation carries no
  version). The wire shapes are `crates/jarvis-protocol/src/tool_api.rs`; the projection is
  `ToolPipeline::policy_inventory` and `::preview_call`.

## Context

`ADR-0122` made a tool's approval policy configurable in the daemon's document: a ceiling, a threshold, a
denial list, and per-tool overrides that can only tighten. That created a **new question** and no way to
answer it.

A configuration document is the **input**. The workspace policy is what the engine enforces. The two are
not the same value: an override is applied as a `max` at evaluation time, a denial short-circuits before
several checks, and the actor's scopes are derived rather than configured. So a document could be written
correctly and still not describe the posture in force, and the operator asking *"did my configuration take
effect"* had no way to find out short of attempting a call and reading the refusal.

Two facts decided the shape of the answer.

**The existing discovery surface is deliberately wrong for this.** `ToolRegistry::discover()` returns
`ToolSummary`, which omits `required_scopes` and `approval` on purpose: a model selects on what a tool does
and must not act on authorization. An operator needs exactly those omissions. Reusing the summary would
either leak authorization vocabulary into a model-facing shape or leave an operator unable to see the
posture they configured.

**`evaluate` is a pure function of declared facts.** Nothing about a decision requires the call to happen,
so the interesting question — *what would this decide* — is computable without running anything. That is
what makes a preview possible at all, and it is also what makes it a **decision** rather than an estimate.

## Decision

**1. The list reports the declared policy and the effective policy, and names an override as an override.**

`ToolReply` carries `declared_approval` and `effective_approval`, plus `overridden` and `denied`. Both
policies are reported because they differ for a reason worth seeing: a tool held by the workspace
*threshold* is indistinguishable from one held by its own *declaration* unless both values are visible. The
reply also carries the workspace's `max_risk` and `approval_threshold`, so one tool's posture is readable
against the ceiling it sits under.

`overridden` is **derived** rather than stored, so the flag cannot disagree with the two policies it
describes. A stored flag would be a third value to keep in step.

**2. A preview is a decision, not a prediction, and it writes nothing.**

`ToolPipeline::preview_call` calls the same `evaluate` the tool-call path calls, over the same workspace
policy and the same definitions. It takes no `run_id` and touches no table: a preview that recorded a call
would let an operator fill the ledger by looking at it, and one that consumed an idempotency key would make
the real call a duplicate. The property is asserted by counting `tool_calls` rows before and after, because
a preview that recorded a `requested` row would look identical in its own response.

**3. The reply carries the reason code, because the outcome alone is unusable.**

The remedy differs per reason: a missing scope is a grant to add, a denial is a configuration line to
remove, an approval is a human to find. A reply that reported only `deny` would leave the operator
reproducing the decision by hand — the defect `PolicyDecision`'s reason code exists to prevent.

**4. A caller supplies context; the daemon supplies authority.**

A preview body carries only the `channel`, the `claimed_strength`, and the `escalation` signals — the
properties of the call only the caller knows. Scopes, the workspace policy, and the definition come from the
daemon, and `deny_unknown_fields` makes an attempt to name them a `422` rather than an ignored value. A
preview must not become a way to ask *"what if I had different permissions"*.

The identity is the profile's seeded local identity with the scopes the registered tools require, which is
the same derivation `call_remote_tool` uses, and for the same reason: the grant is what the tools need, not
what the caller asks for.

**5. Three statuses stay distinct, because the remedies differ.**

- **An unknown tool is `404`, not a refusal.** A typo and a policy denial have opposite remedies, and
  reporting a refusal for a name that does not exist makes them indistinguishable.
- **No tool surface at all is `404`, not an empty list.** "Nothing is configured" and "this daemon cannot
  serve tools" are different deployment facts.
- **A denied tool is `callable: true`.** The denial is a policy fact rather than an availability one, and
  conflating them would send an operator to fix a grant when the remedy is a configuration line.

**6. The closed sets are `jarvis-core`'s types on the wire, not `String`s.**

`Risk`, `ApprovalPolicy`, and `EscalationSignal` appear directly in the DTOs, so a mistyped policy is a
`422` naming the field rather than a silently defaulted value — and a default here would be the *most
permissive* reading of a value a caller mistyped, which is the one direction this surface must never fail
toward. This is the concrete payoff of `ADR-0122`'s move: `jarvis-protocol` depends on `jarvis-core` and
**not on `jarvis-tools`**, so a `String` field plus a hand-written membership check would have been a second
statement of each set. Three such checks were written and then deleted once the move made the types
nameable.

`AuthenticationStrength` stays a name on the wire, deliberately: the strength vocabulary belongs to
`jarvis-tools`, and core's `AuthenticationStrength` is a **different type** meaning what an answering
channel *did* establish. Substituting it would let a ceiling be recorded as an observation.

## Consequences

- **An operator can answer "did my configuration take effect".** The question `ADR-0122` created is
  answerable from a running daemon, which is what makes a configurationurable policy usable rather than
  merely present.
- **A policy change can be inspected before it is relied on.** Because the preview is the same function the
  call path uses, "what would happen if I set this" is answered by the code that decides, not by
  documentation that describes it.
- **The CLI is the second consumer, so the surface is not test-only.** `jarvis tools list` and
  `jarvis tools preview` render both routes, which is the consumer a control-plane UI would replace or join.
- **`jarvis-protocol` gained a module that depends on core types directly.** This is the intended direction:
  a wire contract names domain vocabulary rather than restating it.
- **The preview cannot express a hypothetical authority.** A caller cannot ask what a different scope set or
  workspace policy would decide, which is a limitation by design — a preview that accepted those would be an
  authorization oracle.
- **The list is unbounded.** `MAX_REGISTERED_TOOLS` bounds the registry, so the reply is bounded by
  construction rather than by a page parameter. Pagination would be a later decision if the bound rises.
- **`ToolSummary` is unchanged.** The model-facing surface keeps omitting authorization, so the two
  consumers read two shapes rather than one shape being widened for both.

## Alternatives

- **Reuse `ToolRegistry::discover()` and add the missing fields to `ToolSummary`.** Rejected: it would put
  `required_scopes` and `approval` into the shape a model reads, which is the one place they must not be —
  a model must not reason about authorization, and `ToolSummary`'s own documentation says so.
- **Have the route recompose the workspace policy from configuration.** Rejected: the pipeline holds the
  policy that `call_tool` decides with, and a second composition could disagree with it. The value a client
  is shown would then be one nothing enforced.
- **Record a preview as a `tool_calls` row.** Rejected: it would let an operator fill the ledger by looking
  at it, and it would consume an idempotency key so the real call becomes a duplicate.
- **Return `deny` for an unknown tool.** Rejected: it makes a typo indistinguishable from a policy denial,
  and the two have opposite remedies.
- **Return an empty `200` when no pipeline exists.** Rejected: it reads as "no tools configured" rather than
  "this daemon cannot serve tools", and the remedies differ.
- **Accept scopes or a workspace policy in the preview body.** Rejected: it would make the preview an
  authorization oracle — a way to ask what a different set of permissions would decide.
- **Report a single `approval` field as "the policy".** Rejected: it could only be the declared or the
  effective value, and an operator could not then tell whether their override took effect or the tool's
  author had chosen the posture.

## Conditions that would justify revisiting

- **A control-plane UI** that edits policy at runtime, which would make this surface a read **and** a write,
  and would need the durable workspace policy row and reload path `ADR-0122` records as absent.
- **A measured need to preview against a hypothetical authority** (for example an administrator checking
  what a colleague's grants would decide), which is a genuine question but a different one, and would need a
  separate surface with its own authorization rather than an extension of this one.
- **A registry large enough that the reply needs pagination**, which would add a page parameter and a
  truncation signal — the same shape `DiscoveryReport` already carries for its own bound.
- **A second consumer that needs the preview to include the arguments' effect** (for example showing which
  files a path argument would touch), which would require a target classifier this layer deliberately does
  not have.
