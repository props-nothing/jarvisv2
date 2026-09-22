# ADR-0017: Policy evaluation is a pure function in the adapter crate, with three outcomes and deny overrides

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-003` requires "deterministic policy evaluation with deny-overrides, workspace grants, actor
identity, channel constraints, and reason codes". Four decisions were needed, and three of them
turned out to be about **where a thing lives** rather than what it does.

**First: which crate owns the decision.** `docs/architecture/repository-layout.md` gives
`jarvis-core` "policy decisions and reason codes" and lists `jarvis-tools` as an adapter owning
"registry, policy pipeline, MCP, sandbox ports". The dependency diagram is unambiguous: `Adapters -->
Core`, and "No arrow may point from core/application into a provider adapter."

But the decision must read a `ToolDefinition` — and that type contains two `ToolSchema` values, which
contain compiled `jsonschema::Validator`s. Putting the decision in `jarvis-core` therefore means
either moving a JSON Schema dependency into core (which that crate's own rule forbids: "No Axum,
SQLx, Tauri, MCP SDK, or vendor SDK") or making core depend on an adapter (which the diagram
forbids). The two documents' requirements cannot both be satisfied literally.

**Second: how many outcomes exist.** The obvious design is allow/deny. But
`docs/architecture/security.md` says: "Voice confirmation alone is insufficient for risk-3 actions by
default. A trusted desktop/mobile/CLI approval may resume a voice-originated run." That describes an
outcome that is neither: the call is not permitted and not refused, and a stronger channel can
release it.

**Third: how "deny overrides allow" becomes a property rather than a review habit.** The phrase
appears in both `identity-and-workspaces.md` and `security.md`, with the addition "Missing or stale
evidence fails closed."

**Fourth: how context raises risk.** `tools-and-connectors.md`: "Context can raise risk but cannot
lower a hard policy floor", with examples including an unusually large recipient set and an external
domain.

## Decision

**1. The decision lives in `jarvis-tools`, and the reason it is not duplicated into `jarvis-core` is
recorded rather than silently chosen.** The decision is a pure function over a borrowed tool
definition, so it needs no I/O, no clock, and no persistence — which is what actually makes it
testable as a table. `jarvis-core` keeps `ErrorCode::Authorization`, which is the *transport-facing*
category; the policy reason codes are a finer vocabulary that only makes sense alongside the tool
contract they are about. A later slice that needs an adapter-free policy core can introduce a
`jarvis-core` trait whose implementation reads the contract; the decision logic would move and the
vocabulary would not have to.

**2. Three outcomes: `Allow`, `RequireApproval`, `Deny`.** A hold is not a refusal. Collapsing them
would either make the documented voice resume path unreachable (if a hold were a denial) or let a
voice command delete something (if a hold were an allowance). Every non-allow decision carries a
`DenyReason` whose `is_refusal()` distinguishes the two, so a caller can store a refusal as terminal
and a hold as a pending obligation.

**3. Deny overrides is a control-flow property: checks are ordered, and the first refusal ends the
evaluation.** There is no accumulation of permissive findings, so no later check can fail to outweigh
an earlier one — a structure that makes the rule equivalent to deny-overrides rather than merely
consistent with it. The order is fixed and documented: availability, actor status, explicit workspace
denial, workspace ceiling, scopes, tool policy, approval obligation, authentication strength. The
denials (steps 3, 5, 6) deliberately precede the approval steps, so a call that is denied **and**
would need approval reports the denial rather than sending an operator to approve something that
cannot run.

**4. A channel caps the authentication an actor may claim.** `channel_ceiling` is the mechanism behind
the voice rule: a voice channel can establish `ChannelEvidence` and nothing more, so a risk-3 call
from voice is held for an approval that must arrive through a channel whose ceiling reaches
`Present`. The client's *claimed* strength is capped by the ceiling rather than trusted, so a client
cannot raise its own strength by asserting one. The documented outcome falls out of the ordering
rather than needing a special case.

**5. Context escalates by a maximum, and the escalation is recorded as typed signals.** Risk is a
level of scrutiny, not a quantity, so escalation is `max` and never a sum. The signals are one closed
enum used both as the assessment's input and the decision's record — which also resolved a real
duplication, since the first implementation had a struct of four booleans **and** an enum for the same
information, and nothing kept them in step.

## Consequences

- The decision is a pure function of declared facts and supplied context. No clock, no repository, no
  network — so the policy table is testable directly, and a decision is reproducible from its inputs.
- A workspace policy can only make an outcome **more** restrictive than the tool's own declaration
  and the actor's grants. A denial in any source wins, and the tool's own `Deny` cannot be relaxed.
- An operator can answer "why was this refused" from a stored record, and a model told `denied` gets
  a code it can act on rather than a bare boolean.
- A held call always reports the strength that would release it, so a caller cannot hold a call
  without knowing what would let it proceed. Supplying that strength — an authenticated approval with
  a nonce and expiry — is `P3-004`.
- `jarvis-core` does not gain a JSON Schema dependency, and the adapter direction is not inverted.
  The cost is that the decision is not in the crate the layout names first; ADR-0017 and this
  reasoning are the record of why, so the next reader does not rediscover the conflict.
- An unreachable-`&'static str` in a serialized record was rejected in favour of a typed enum, so a
  stored escalation signal cannot be a free-form string and cannot silently change meaning on a
  rename.

## Alternatives considered

- **Put the decision in `jarvis-core`.** Rejected for the dependency reason above. Not silently: it is
  recorded as the tension between two documents rather than resolved by ignoring one.
- **A boolean decision.** Rejected: it cannot distinguish a hold from a denial, and it gives an
  operator and a model nothing to act on.
- **Boolean `allowed`/`requires_approval`.** Rejected: two booleans represent an impossible state
  (`not allowed` and `requires approval`) and lose the reason entirely.
- **Let workspace policy relax a tool's approval policy.** Rejected: a workspace setting would become a
  way to remove a guard the tool author declared, and the direction of the whole model is that a
  workspace narrows and never widens.
- **Sum escalation levels.** Rejected: three signals would demand a risk level that does not exist,
  and the mapping from signals to posture would stop being statable.
- **Trust the client's claimed authentication strength.** Rejected: a client asserting `Present` over
  a voice channel would defeat the voice rule, and the channel is the thing the platform can actually
  observe.
- **Refuse a voice-originated risk-3 call.** Rejected against the documented resume path; the call is
  held instead, which is what the document describes.
- **Escalate from `ApprovalPolicy` rather than from context.** Rejected: the escalation examples are
  facts about this call (how many recipients, which domain), which a static declaration cannot carry.
- **A `Vec<&'static str>` for the escalation signals.** Rejected by a compile error on `Deserialize`,
  and replaced with a typed enum — which also removed the struct/enum duplication.

## References

- [docs/architecture/tools-and-connectors.md](../architecture/tools-and-connectors.md)
- [docs/architecture/identity-and-workspaces.md](../architecture/identity-and-workspaces.md)
- [docs/architecture/security.md](../architecture/security.md)
- [docs/architecture/repository-layout.md](../architecture/repository-layout.md)
- [ADR-0015: Tool contract consistency and offline schemas](0015-tool-contract-consistency-and-offline-schemas.md)
- [ADR-0016: Supplied schema documents and registry boundaries](0016-supplied-schema-documents-and-registry-boundaries.md)
