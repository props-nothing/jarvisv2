# ADR-0025: An MCP server's effects and risk come from an operator, never from the server

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-008a` established what an MCP tool is *called* and which server-supplied schemas may be offered.
`P3-008b` had to answer the harder question: **what may a server's tool do, and at what risk?**

An MCP server supplies `name`, optional `title`, optional `description`, `inputSchema`, optional
`outputSchema`, and an optional `annotations` object. It supplies **nothing** about effects, risk,
approval policy, or scopes. And it cannot be asked to: the server is the party whose behaviour is in
question.

The specification makes the same point from the other side. Of `annotations` it says, in a warning:
"clients **MUST** consider tool annotations to be untrusted unless they come from trusted servers." The
annotations carry hints named `readOnlyHint`, `destructiveHint`, and `idempotentHint` — and those are
exactly the three facts JARVIS's policy engine needs.

So there are two candidate sources for a tool's posture, and one is a self-report by the tool.

There is also a subtlety that makes the self-report worse than merely unreliable. `EffectSet::risk_floor`
is a **maximum**, not a sum, and it gives `ExternalCommunication` a floor of 2 and `Destructive`,
`Financial`, `Privileged`, and `CodeExecution` a floor of 3. `Risk::High` maps to
`ApprovalPolicy::Ask`. That means a server that admits *one* outward effect can never be silently auto
approved — the ladder leaves no such rung. A server that under-reports has no such problem.

## Decision

**1. Effects, risk, and approval come from [`ToolEffectPolicy`], an operator's statement.**
The listing's `annotations` field is **not read at all**. `McpToolListing` has no field for it, so the
translation cannot consult it even by accident, and a test asserts that smuggling annotations into the
input schema changes nothing about the resulting definition.

**2. A server nobody has classified gets a posture that makes every failure a refusal.**
`ToolEffectPolicy::unclassified()` declares `ExternalCommunication` **and** `Write` — a risk floor of 2
— and then declares risk 3 with `ApprovalPolicy::Ask`, which workspace policy cannot lower the way
`Policy` could. No automatic retry, and `Idempotency::Unsupported`.

The asymmetry decides this: a read-only server is mildly inconvenienced by an approval prompt, while a
mass-mail server presumed harmless is not inconvenienced at all. Erring toward a prompt costs a click;
erring toward permission costs an effect nobody approved.

**3. `read_only()` is the one convenience constructor**, because "this server only reads" is the
common case and the difference is stark: risk 0, `Auto`, and a legal retry — legal precisely because
repeating a read has no second effect.

**4. Effects are an operator statement rather than a schema inference.**
It is tempting to derive them: "it takes a `query` string, so it reads." That is inference presented as
knowledge, and this project's rule is that unsupported inference is not persisted as fact. A tool with
no declared posture is *unclassified*, and the definition says so.

**5. The JSON Schema dialect is supplied, refused, or never defaulted silently — depending on who
omitted it.**
MCP says a schema with no `$schema` "defaults to 2020-12". `jarvis-tools` **requires** the keyword and
refuses to default it, for the good reason that `exclusiveMinimum` is a boolean in earlier drafts and a
number in 2020-12, so a correct older-draft document would silently become an invalid newer one.

Both positions are right and they compose: a server's **omission** is supplied with 2020-12, because
that is what the protocol says the omission means; a server's **declaration of a different dialect** is
refused rather than reinterpreted, because reinterpreting it is exactly the silent-translation defect
`jarvis-tools` refuses. The refusal names both dialects so an operator can act.

**6. A server's `title` and `description` are sanitised before they reach a model.**
Whitespace collapses to single spaces; control characters are dropped; and **bidirectional and
zero-width formatting characters are removed**. The last is not typographic tidiness: a Unicode
bidirectional override changes how text *reads* without changing what it *is*, which in the field that
decides whether a tool gets called is a deception primitive. Both are bounded, and a missing
description says "the server supplied no description" rather than inventing a plausible one.

## Consequences

- A translated definition's effects, risk, and approval are auditable facts about an operator's
  configuration rather than claims by a remote process. An operator can answer "why was this call
  approved" without consulting the server.
- Because `EffectSet::risk_floor` is a maximum, an operator cannot declare a posture that *hides* an
  effect: `ToolEffectPolicy::new` refuses `risk` below the floor, at configuration time rather than when
  a remote server is mid-call. The same constructor refuses a retry declaration that would repeat an
  un-deduplicated outward effect, so a policy that would send a payment twice cannot be *held*.
- `translate_listing` excludes per tool rather than failing the list, for names as well as schemas. A
  name collision is an **exclusion, never a rename**: any rename is a mapping the operator did not
  choose, and the operator is who can decide which server to call something else.
- The derived `version` is `schema-<8 hex>` over the **input schema**, so a cosmetic edit to a
  description does not invalidate a stored intent while a schema change does. It is a digest rather than
  a counter because a version must be reproducible across a restart.
- `ToolEffectPolicy::new` takes a `ToolEffectPolicyParts` struct rather than eight parameters. Clippy's
  `too_many_arguments` was right, not noise: several of the eight are adjacent short values a caller
  could transpose silently — `risk` and `timeout_seconds` are both numbers. This is the same reasoning
  `P3-005` applied to its call parts.

## Limits, stated rather than implied

- **Still nothing has been exercised against a real MCP server.** These are pure functions tested as
  pure functions, and the field names come from the recorded specification rather than from bytes.
- **No configuration surface exists.** `ToolEffectPolicy` cannot be built from `config.toml`, so in
  practice every MCP server would currently get `unclassified`. That is the safe direction — every call
  held for approval — but it means `read_only()` has no caller outside tests, and the operator story is
  incomplete until `P3-009` or a dedicated slice adds the keys.
- **The scope vocabulary is a single coarse `mcp.call`.** An MCP server is treated as one trust unit.
  There is no per-tool or per-server scope, because there is no grant model that could express one yet;
  inventing a naming scheme with no users would be worse than one honest scope.
- **A tool that declares no output schema gets a permissive one.** That is the truthful encoding of "the
  server made no claim", but it means an unvalidated result reaches the model. The bound that protects
  it is the output-size limit in the execution path, not schema validation.
- **The version does not include the tool's name.** Two servers' identically-schemaed tools share a
  version string. Harmless for the intent hash (which covers tool, version, and arguments), but worth
  not mistaking for a per-tool revision.
- **`Idempotency` is never `Required` or `ProviderKey` for an MCP tool**, because MCP has no way to
  express provider-side deduplication. So an outward MCP call can never be retried automatically, which
  is safe but means a transient network failure is always a failure.
- **`translate_listing` builds its own `NameAssignments` per call**, so it cannot detect a collision
  between two *different* servers. That is the aggregate's job, and the test drives the cross-server
  case directly rather than pretending one listing call sees two servers.

## Alternatives considered and rejected

- **Read `annotations.readOnlyHint` and map it to effects.** Rejected: the protocol warns it is
  untrusted, and it is a self-report by the party whose behaviour is in question. It is also the one
  field where an attacker's incentive and the safe answer are furthest apart.
- **Infer effects from the input schema's shape.** Rejected: inference presented as knowledge. A schema
  with a `query` string could read or could send; the schema does not say which.
- **Default to `read_only`.** Rejected: it makes the common case pleasant and the failure case an
  unaudited outward effect. The default must fail closed.
- **Default to `unclassified` but with `Policy` rather than `Ask`.** Rejected: `Policy` is a posture
  workspace policy can lower, so an unattended workspace could auto-approve an unclassified remote tool.
  `Ask` cannot be lowered.
- **Refuse a server whose tools have no operator-declared posture.** Rejected: it makes the integration
  unusable by default, and a held call is a better outcome than no tool — the row records that an
  approval was needed, which is information.
- **Default a missing `$schema` to 2020-12 unconditionally, as MCP says.** Rejected: it would make
  JARVIS reinterpret a document whose author wrote it against another draft. Supplying on *omission*
  and refusing on *contradiction* is the reading that respects both documents.
- **Silently strip a `$schema` that contradicts JARVIS's dialect.** Rejected: it is the same
  silent-translation defect, and the operator would never learn the server is speaking a different
  dialect.
- **Keep the eight-parameter constructor and `#[allow]` the lint.** Rejected: the lint was pointing at
  a real transposition hazard, and the grouping costs one struct.
- **Preserve a server's description verbatim.** Rejected: a bidirectional override in text that decides
  whether a tool is called is a way to misrepresent the tool at the exact moment of the decision.

## Revisit if

- `P3-009` adds the configuration keys, at which point `read_only()` and the risk/approval defaults
  become operator-visible and their ergonomics can be judged rather than assumed.
- A real server turns out to supply a `$schema` value JARVIS should accept, at which point the dialect
  refusal needs a mapping rather than a string comparison.
- MCP gains a way to express provider-side deduplication, at which point `Idempotency::ProviderKey`
  becomes available to an MCP tool and the no-automatic-retry limit can be lifted where it is safe.
- An operator needs a posture that is neither `unclassified` nor `read_only` — a write-only server, for
  instance — at which point a configuration surface matters more than the two constructors.
