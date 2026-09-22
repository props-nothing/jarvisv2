# ADR-0015: Tool contracts carry their own authorization inputs, and a schema is never fetched

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-001` defines the canonical tool contract: "identifiers, JSON Schema 2020-12 input/output,
effects, risk, scopes, source, version, timeout, retry, and idempotency metadata". Three decisions in
that work are durable — they constrain every later tool slice, are visible in stored data, and would
be expensive to reverse once tools are registered. Each needed a decision rather than an
implementation choice.

**First: how tool identity is formed, and what it implies.** `docs/architecture/tools-and-connectors.md`
requires that "tool names are namespaced and collisions are errors" and separately lists a `source`
field (`native`, `connector`, `MCP`, `runtime`, `extension`). Those could be two independent fields,
or one field derived from the other. The difference matters because the source class decides how
third-party output is treated, and a tool that could *declare* its own class could claim to be native.

**Second: whether schema validation may resolve references.** JSON Schema's `$ref` can point at a URL
or a file, and the obvious library configuration resolves them. Tool schemas are authored by JARVIS
and by **connector manifests**, and `docs/architecture/security.md` lists SSRF as a threat with
"URL policy, TLS, auth, SSRF defenses, and egress controls" as the controls. A schema that fetched a
URL during validation would be an outbound HTTP client whose request target a connector chose.

**Third: when cross-field consistency is enforced.** Risk, effects, retry, and idempotency constrain
each other: `docs/architecture/tools-and-connectors.md` gives risk 3 the posture "delete, spend,
deploy" and risk 0 "read calendar", and says automatic retry of `unknown` "is forbidden for
non-idempotent effects". Those relationships can be enforced when a tool is **constructed**, when a
call is **made**, or documented and trusted. They interact with the second decision, because a
manifest is the first thing that arrives from outside.

## Decision

**1. Tool identity is `namespace.name`, split on the last dot, and the source is derived from the
namespace rather than declared.** A dotted namespace is a qualified path (`mcp.github.search` has
namespace `mcp.github`), and a marker matches a whole segment or a leading segment, never a partial
one, so a connector named `mcpfoo` is not attributed to MCP. A name may not contain a dot, which is
what keeps the printed string and the parsed halves from disagreeing. A definition that declares a
`source` disagreeing with its identifier is refused.

**2. Schema validation never resolves an external reference, enforced twice.** Any `$ref` or `$id`
that is not a local fragment (`#...`) is refused by name before the validator is built, **and** the
validator is constructed with the resolver disabled (`.offline()` over a `jsonschema` dependency
pinned with `default-features = false`). The refusal is its own error variant so the reason is
reported as a refusal rather than as invalid syntax.

**3. Risk, retry, idempotency, and effects are checked when the definition is built — including
after a manifest is parsed.** A declared risk below the effect floor is refused; a blind retry for a
non-idempotent mutating effect is refused. The unvalidated manifest shape is a separate type
(`ToolDefinitionParts`, plus `RetryDeclaration` for the one field that needs inputs the manifest does
not carry), so the check runs on the path from JSON as well as the path from Rust code. Effect sets
may not be empty, and that survives deserialization.

**4. Scope vocabulary is JARVIS capabilities, not provider OAuth scopes.** `required_scopes` is
`resource.action` (optionally `resource.*`), which JARVIS can grant. A provider OAuth scope string
belongs to the connector's token lifecycle and is not representable here, because a tool requiring
one could never be satisfied by the JARVIS authorization model.

## Consequences

- A tool cannot misdeclare where it came from. Third-party handling keys on a value derived from the
  registry key rather than on a field the manifest controls.
- A connector cannot make JARVIS fetch a URL by writing a schema. The cost is real and accepted: a
  schema needing a shared definition must inline it or resolve against documents JARVIS supplies. A
  tool declaring such a reference is refused at registration with a message naming the reference.
- An inconsistent declaration cannot be stored. `P3-002`'s registry can rely on a registered tool's
  risk and retry being consistent with its effects, because construction is the only door — and this
  holds for a manifest, not just for hand-written code.
- `RetryPolicy` is deliberately not `Deserialize`. A stored policy is a `RetryDeclaration` until it
  is validated against the tool's effects and idempotency.
- A capability wildcard is representable but does not cross resources: `files.*` does not cover
  `gmail.send`. Whether an actor may *hold* a wildcard is `P3-003`'s decision, not this contract's.
- The contract is inert data. Nothing here decides whether a tool may run in a workspace; that is the
  deterministic policy engine's job, which reads these fields rather than a description string.

## Alternatives considered

- **A declared `source` field checked against nothing.** Rejected: it is exactly the field a tool
  would misdeclare, and the registry key would then disagree with the trust classification.
- **Matching `jarvis` by equality and other markers by prefix** (the first implementation). Rejected
  once tested: it classified `jarvis.files.read` as a connector while classifying `mcp.github` as
  MCP — the same namespace shape, two answers, because one marker happened to be an equality test.
- **Enabling `jsonschema`'s default features with a URL allowlist.** Rejected: an allowlist is a
  policy that has to be maintained and can be widened by mistake, whereas not having a resolver is a
  property. It also removes `reqwest`, `rustls`, and `idna` from the graph.
- **A hand-rolled keyword allowlist instead of a JSON Schema implementation.** Rejected: it would be
  a partial validator whose gaps are invisible, and a schema that "validated" would mean less than the
  declared dialect says.
- **Validating risk against effects at call time instead of construction time.** Rejected: the
  inconsistency would be discovered while a user waits for an approval prompt rather than at
  registration, and a stored manifest would be inconsistent in between.
- **A `u8` risk instead of a typed level.** Rejected: the approval posture is chosen from the level,
  and `7` would have silently behaved as level 3 while every table lookup missed. Out-of-range is
  refused where the level is parsed.
- **Asserting JSON Schema `format`.** Rejected and recorded as a decision: format checking covers only
  the formats the library implements, so a tool declaring `"format": "email"` would silently accept
  what it expected to refuse. A tool constrains shape with `pattern`; reachability of an address is
  something only the provider can establish. There is a test fixing this so it cannot change quietly.

## References

- [docs/architecture/tools-and-connectors.md](../architecture/tools-and-connectors.md)
- [docs/architecture/security.md](../architecture/security.md)
- [docs/architecture/identity-and-workspaces.md](../architecture/identity-and-workspaces.md)
- [docs/research/integrations/json-schema-validation.md](../research/integrations/json-schema-validation.md)
- [ADR-0005: Canonical tools and MCP](0005-canonical-tools-and-mcp.md)
