# ADR-0122: An operator's approval override can only tighten, and the risk vocabulary moved to core

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** configurable tool approval policy — making the per-tool policy the engine already honored
  reachable from an operator's configuration document, so a tool's approval can be configured without a
  rebuild.
- **Relates to:** `ADR-0017` (policy evaluation is a pure function, deny overrides allow, and workspace
  policy may not relax a tool's declaration), `ADR-0023` (the tool pipeline is composed in the daemon),
  `ADR-0121` (a configuration surface references rather than holds what it must not republish),
  `ADR-0059` (a tool definition is derived from a connector manifest). The evaluation contract is
  `crates/jarvis-tools/src/evaluation.rs`; the risk and approval vocabulary is now
  `crates/jarvis-core/src/risk.rs` and `approval_policy.rs`.

## Context

The tool pipeline already honored a per-tool approval policy. `ApprovalPolicy::Ask` was checked
unconditionally in `evaluate`, a workspace could deny a tool by identifier, and `WorkspacePolicy::new`
refused a threshold above its ceiling. What did not exist was any way for an **operator** to say any of
it: `WorkspacePolicy::default()` was constructed at the composition site, `denying()` had no production
caller, and the configuration document had no policy section at all. So the vocabulary was built and
tested and the surface to use it was absent — the "producer with no consumer" shape this repository
treats as a defect wherever it finds it.

Building the surface surfaced one constraint that decided the whole shape.

**`jarvis-storage` cannot depend on `jarvis-tools`.** `docs/architecture/repository-layout.md`'s
dependency graph allows an adapter to depend on `jarvis-core` and `jarvis-protocol` and **not on another
adapter**. Configuration parsing is `jarvis-storage`'s role and authorization is `jarvis-tools`'s, so a
`PolicyConfig` holding a `Risk` or an `ApprovalPolicy` could not name `jarvis-tools`'s types.

The obvious workaround was a second risk vocabulary in `jarvis-storage` — a `u8`, or a local enum. That is
the defect class this repository keeps finding: two values that must agree with nothing holding both. A
storage-side approval level that drifted from the engine's would be a policy an operator configured and
the engine silently did not honor, and the drift would be invisible because both would compile.

## Decision

**1. `Risk` and `ApprovalPolicy` move to `jarvis-core`, and `jarvis-tools` re-exports them.**

They are **one vocabulary with one meaning** needed by two adapter crates that may not depend on each
other. `jarvis-core` is the crate both already depend on, and it already holds `Sensitivity` for the same
reason. `jarvis-tools` re-exports both, so every existing `crate::Risk` and `crate::ApprovalPolicy` path
keeps resolving and there is still exactly one definition.

**The floor check moves its argument, not its home.** Core's `Risk::declared_for` takes the effect floor
as a **number**, because an effect set is `jarvis-tools`'s type. `jarvis-tools::declared_for_effects` is
the single binding that supplies `EffectSet::risk_floor()`, so "a financial tool cannot be risk 0" still
has exactly one home in the crate that owns effects.

**2. An override is a `max`, applied at evaluation time and never at construction time.**

`WorkspacePolicy::requiring(id, policy)` stores what the operator wrote. `evaluate` calls
`effective_approval(definition.id(), definition.approval())`, which returns the **tighter** of the two via
`ApprovalPolicy::tighter` — a `max` over `strictness()`. Applying it at construction instead would make
the stored value the effective value, which is the relaxation `ADR-0017` rejects by name.

**3. The comparison is a named method, not an inline comparison, because the direction is the security
property.**

`ApprovalPolicy::strictness()` gives `Auto = 0, Policy = 1, Ask = 2, Deny = 3`, and `tighter` returns the
higher. A predicate like this **reads correct in both directions** — `tighter` would be a plausible name
for "return the looser operand" if the doc comment explained it fluently enough. Only a concrete case
settles it, so the tests state the adversarial case in their names: *"a workspace override must not relax
a tool that declares Ask"*, and a sweep asserting the property for every `(declared, override)` pair.

**4. `Ask` is stored as written; it is not translated to `Deny`.**

The first implementation *did* translate, "for safety" — reasoning that an override must be unable to lose
to a tool declaring `Deny`. It was the defect. Because the override is a `max`, `tighter(Deny, Ask)` is
already `Deny`, so no translation was needed; and the translation broke the common case, making "hold this
tool for approval" refuse the call outright. A test named for the tightening direction caught it — *"a
stricter override must win, or the override does nothing"*. **A second guard that also changes the meaning
is not a guard**, and the `max` was already the mechanism.

**5. `PolicyConfig` is a document section, and its absence is the default.**

`[policy]` carries `max_risk`, `approval_threshold`, `deny`, and `[policy.approval]` as a table of
`id = "policy"`. Each field is optional and each falls back to the workspace default independently, so a
document that configures only one of the two risk fields gets a legal policy rather than a contradiction
with a default nobody wrote. The section is `#[serde(default)]`, so a document written before it existed
still parses — the safe direction, because an absent section means "no operator opinion" rather than a
refusal to start.

**6. The document refuses self-contradiction, and does not refuse an unknown tool.**

A threshold above the ceiling is refused at parse time as well as in `jarvis-tools`, so the error names
the configuration file an operator edited rather than a crate they have not read. A blank tool identifier
is refused, because it applies to nothing while reading as a configured restriction.

A tool identifier that names **no registered tool** is deliberately **not** refused. The registry is not
reachable from `jarvis-storage`, and an MCP server's tools are discovered at startup: an operator who
wrote a policy naming a server that is temporarily down would otherwise be unable to start the daemon. An
inert entry is the honest outcome, and `compose_workspace_policy` is where the identifier is parsed and
where an *invalid* one is reported by name.

## Consequences

- **An operator can configure approval per tool.** `[policy.approval] "jarvis.mail.send" = "ask"` holds a
  call the workspace would otherwise allow, and `deny = [...]` refuses one outright — the vocabulary the
  engine already honored is now reachable from the document.
- **The direction rule holds at two seams, not one.** The engine applies the `max` at evaluation, and the
  composition root translates through `requiring` rather than writing the operator's value in. The rule
  lives with the policy, so a future runtime editor inherits it rather than reimplementing it.
- **`Risk` and `ApprovalPolicy` now have one home.** A stored risk, a configured risk, and a declared risk
  are the same type, so a value cannot be configured with one vocabulary and interpreted with another.
- **The tool-identifier vocabulary stays split, deliberately.** `PolicyConfig` holds `String`s and the
  composition root parses them, because `ToolId` belongs to `jarvis-tools`. The cost is that a malformed
  identifier is a startup error from the composition root rather than a parse error, which is recorded
  rather than hidden.
- **There is still no runtime policy surface.** The document is the seed and is read at startup; a
  control-plane UI that edits policy needs a workspace row plus a reload path, which is a separate slice.
  `ADR-0121`'s convention applies to that work: the document references what it must not republish.
- **`ADR-0017`'s rejection is now enforced rather than merely documented.** It was always the intent; a
  `max` with a named method and a sweep test is what makes it a property of the code instead of a
  paragraph in a record.

## Alternatives

- **A second risk vocabulary in `jarvis-storage`.** Rejected: two values that must agree with nothing
  holding both, and the drift would be silent because both would compile.
- **Have `PolicyConfig` hold a `u8` and parse it in the composition root.** Rejected: it moves a
  validation the type can carry into a layer that would have to remember to run it, and a bad value would
  become a startup error with a worse message rather than a parse error naming the field.
- **Apply the override when the policy is built rather than at evaluation.** Rejected: it makes the stored
  value the effective value, so the override *replaces* the declaration and a looser override removes a
  guard — exactly what `ADR-0017` rejects.
- **Refuse an override for a tool that is not registered.** Rejected: MCP tools are discovered at startup
  and a server may be down, so this would make a valid policy refuse to start. An inert entry is the
  honest outcome.
- **Keep `Risk` in `jarvis-tools` and give `jarvis-storage` a `jarvis-tools` dependency.** Rejected: the
  dependency graph forbids an adapter depending on another adapter, and that rule exists because a storage
  crate that can see the tool registry is a storage crate that will eventually read it.

## Conditions that would justify revisiting

- A **runtime policy editor** (CLI or control plane) that writes policy while the daemon runs, which needs
  a durable workspace policy row, a reload path, and a decision about whether the document is a seed or a
  projection — and which would move the translation out of `compose_workspace_policy`.
- A **second consumer of `Risk` outside tool authorization** (for example a memory sensitivity ceiling
  expressed as risk), which would confirm the move to core or argue for a third home.
- A requirement to **express a per-tool override for a tool that does not exist yet** in a way that is
  validated rather than inert, which would need the registry reachable at composition and a decision about
  what "declared but unregistered" means.
- A **measured need to relax an override** for a specific tool (for example an operator who deliberately
  wants to auto-run a tool whose author declared `Ask`), which would be a superseding decision to
  `ADR-0017` and would need a reason that survives the guard being removable by configuration.
