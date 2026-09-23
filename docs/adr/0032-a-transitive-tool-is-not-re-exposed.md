# ADR-0032: A transitive tool is not re-exposed, and exposure is not the catalog

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009a` made the origin decision a policy value because the SDK's server defaults were permissive on the
specification's MUSTs. The next question is the one that actually decides whether JARVIS is safe to expose:
**which of our tools may a remote MCP client call?**

The registry is not a small list. `apps/jarvisd` composes a filesystem adapter, MCP adapters for every
configured third-party server, and — as connectors and runtimes land — provider and runtime tools. Most of
that is not ours. Two tempting shortcuts are wrong:

- **Serve everything the registry holds.** The registry is the *local* answer to "what this user can do". A
  remote caller is a different principal, and a set chosen for one principal must not be published to
  another by default.
- **Reuse `McpCatalog`.** It already answers "what can this client reach", and it is the right type for the
  outbound direction. But its rules are the opposite direction's rules, and reading it inbound would give
  the wrong answer in three specific places (below).

## Decision

**1. The served surface is its own type, and it does not share the catalog's rules.**

`served_tools` lives in `jarvis-mcp` beside `McpCatalog`, and the two answer different questions with
different rules:

| | `McpCatalog` (outbound) | `served_tools` (inbound) |
| --- | --- | --- |
| Naming | an operator's local name plus the server's tool name, translated into a canonical identifier | the **canonical identifier verbatim**, because it is already namespace-qualified |
| Posture | an operator's `ToolEffectPolicy` against a third party (ADR-0025) | irrelevant — what matters is whether the **source is code this project wrote** |
| Absence | an unreachable server excludes its tools | a *known but unavailable* tool is excluded, because advertising it offers a tool that fails on every call |

The naming row is why a shared type would be wrong rather than merely redundant. Outbound, rule 1 of this
crate says **a server does not name itself**, so we invent the name. Inbound, the only name we can be sure a
client's operator can use is the canonical identifier — and a tool whose identifier is not transmittable is
**excluded, not renamed**.

**2. A tool whose source is third-party code is never re-exposed, and the refusal is named per tool.**

`ToolSource::is_third_party()` — `Mcp`, `Runtime`, `Extension` — is the test. A connector tool **is**
servable, because JARVIS wrote the adapter and our own operator declared its risk; that distinction is the
reason the rule is `is_third_party()` rather than "not `Native`".

The reason is the module's most consequential claim, so it is stated in the code too: **our exposure
decisions describe *this daemon*.** If a remote caller reaches `mcp.github.search` through us, they have
driven a call through our policy *and then through a third-party tool our policy never classified, in a
context we did not choose*. The `ToolEffectPolicy` governing it was written for *our* use of that server on
*this* machine under *this* user's grants. Re-transmitting it makes that policy a statement about a caller it
was never about.

Nested exposure is deliberately not built. A federation feature has its own unanswered questions — whether
the third party consented, whose credentials execute the call, which audit record owns it — and building it
as a side effect of "expose my tools" is how a trust boundary widens without a decision.

**3. A known-but-unavailable tool is withheld and reported.**

`Availability::reason()` is the signal. Advertising it would offer a remote caller a tool that fails on
every call, which reads as a broken tool rather than an absent capability — the same reasoning
`compose_tool_pipeline` already applies when it returns no pipeline over zero roots.

**4. The most consequential reason wins, and that ordering is the decision.**

A transitive tool that is *also* unavailable is reported as **Transitive**. Falsified: checking availability
first makes the reason "the account is not connected", so an operator reconnects an account and fixes
nothing, because the tool would still not be exposed. Reporting the reason no change to availability could
fix is what makes the report actionable.

**5. An untransmittable identifier is excluded and named, never renamed.**

The protocol **SHOULD**-constrains a tool name to `A-Za-z0-9_.-` and is case-sensitive; a canonical JARVIS
identifier may legally contain `:` in its name segment. So `jarvis.files:read` is a *constructible* tool that
the protocol's alphabet cannot carry. Renaming it would give a remote caller a name the daemon's own
operator cannot find in their configuration — the failure the canonical-identifier work exists to prevent.
The check is deliberately **stricter** than the wire's alphabet: it admits exactly what the canonical rules
produce, so a name this project cannot produce has no business being transmitted and a future identifier
vocabulary cannot become transmittable by accident.

**6. One refusal never removes the others, and the report names each one.**

The specification requires that a single malformed tool definition not prevent other valid tools from being
used. Here the consequence is stronger than a missing convenience, because a `Transitive` refusal is a
**trust decision** — so each withheld tool is named individually with its reason, and a caller can see
*that* something was withheld rather than inferring it from absence.

**7. The truncation is about the list, not a tool.**

`ExposureExclusion::BeyondLimit` is separate and reports a count, and `tool()` returns `None` for it. An
operator fixing one tool's availability would learn nothing from a size bound, which is the same distinction
`McpCatalog` draws between an exclusion and a truncation. The bound is checked **only after every
eligibility check passes**, so `dropped` means "withheld for size" rather than "withheld".

**8. An empty surface is an error rather than an empty list.**

Binding an endpoint that advertises nothing would be a silent no-op; a caller that wants to serve nothing
should not bind. This mirrors the tool pipeline's "no roots means no pipeline, not an empty one".

**9. The list is ordered, and the served schema is the declared one.**

Sorted by identifier, because the specification requires the set not to vary per connection and says the
order **SHOULD** be deterministic — a hash-order list satisfies the first and not the second, and the
difference shows up as a client's tools appearing to change between calls. The schema travels as declared,
so a remote caller sees the contract the daemon actually enforces rather than a summary of it.

## Consequences

- **The transitive refusal is the property a security reviewer should test first**, and it is pinned by
  three falsifications: removing the guard serves `mcp.github.search`; checking availability before source
  changes the reported reason to the unfixable one; removing the bound serves 35 tools instead of 32.
- **Exposure and discovery now have visibly different rules in the same crate**, which is itself the
  documentation. The table above is the reason a shared type was rejected, and it lives in the module.
- **A connector tool's standing is explicit.** `gmail.messages.send` is servable and `mcp.github.search` is
  not, and the boundary is "did this project write the adapter" rather than "is the name reserved".
- Still **not built**, and recorded rather than implied: nothing serves anything yet, because `P3-009b` wires
  the transport to these two values; there is no per-client allowlist or rate limiting (`P3-009c`); the
  served schema is transmitted verbatim, so a schema the protocol's own `x-mcp-header` rules reject is
  **not yet filtered on the server side** (the client-side check in `check_tool_schema` is the model for it);
  and **no nested/federated exposure exists**, deliberately.

## Alternatives rejected

- **Serve everything the registry holds.** Publishes a set chosen for one principal (this user) to another
  (a remote caller) by default.
- **Reuse `McpCatalog` for the inbound direction.** Its naming rule is the opposite of what inbound needs,
  its posture is about a third party rather than about code we own, and an unavailable tool's treatment
  differs. Three wrong answers from one plausible reuse.
- **Rename an untransmittable identifier.** Gives a remote caller a name its own operator cannot find in
  their configuration.
- **Expose transitive tools with their original posture attached.** Makes an operator's guess about someone
  else's code into a statement about a caller it was never about.
- **Build nested exposure now, since the pieces are there.** A trust-boundary widening disguised as a
  feature, with three unanswered questions about consent, credentials, and audit ownership.
- **Report one generic "these tools were excluded" message.** A `Transitive` refusal is a trust decision;
  a caller has to see which capability and why, not a count.
- **Report a truncation as a per-tool exclusion.** The remedy differs — it is about the size of the surface,
  not about any tool.
- **Filter the served schema here.** The `x-mcp-header` constraints are a client obligation in the protocol
  and the server-side filtering is `P3-009b`'s to place, next to the transport that would enforce it.
