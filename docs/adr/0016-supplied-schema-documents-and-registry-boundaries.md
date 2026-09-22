# ADR-0016: A schema composes only against documents JARVIS supplied, and external manifests are self-contained

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-002` requires a capability registry. It has to answer three questions that turn out to be
different: what is registered (and what happens when two tools collide), what can run **right now**,
and what a model is told. It also inherited a debt from `P3-001`.

That debt was stated as a cost in `P3-001` and its research record: refusing every non-local `$ref`
closed an SSRF surface (a schema could fetch a URL, chosen by whoever wrote the schema), but it meant
a schema needing a shared definition had **no** path but inlining. `P3-001`'s own doc comment named
this slice as the one to supply the document set, and `docs/architecture/tools-and-connectors.md`
gives a connector manifest "configuration JSON Schema", which is exactly where a shared definition is
most likely to be wanted. So the question was not whether to relax the rule but **how precisely**.

Two facts were established by running code rather than by reading documentation, because the
documentation did not answer either:

1. With `jsonschema`'s resolver features disabled **and** `.offline()` set, a `$ref` to a document
   supplied through `with_registry` still resolves and its constraints are enforced. So relaxing the
   rule is possible **without** reopening the network path.
2. A `ToolSchema`'s `Deserialize` — the path a stored manifest arrives through — has no document set
   to supply.

The second is the interesting one, because the obvious fix (thread a document set through
deserialization) creates a problem. `$id` is a **document-controlled** string. If a connector manifest
could reference a document, its author would gain influence over *which* JARVIS document is loaded,
and a manifest declaring an `$id` matching a JARVIS URI could shadow the definition it claims to
reference.

A second decision was forced by the registry: whether a tool's availability belongs to the definition
or to the registry. `P3-001` put `availability` in the definition, and `docs/architecture/tools-and-connectors.md`
describes it as "health and configuration requirements" — but `P3-002`'s task explicitly says
**dynamic** availability, which a stored definition cannot be.

A third was whether registration is per-tool or per-manifest.

## Decision

**1. A schema composes against a JARVIS-supplied `DocumentSet`, and nothing else.** `ToolSchema::parse_with`
accepts a set of documents keyed by the `$id` **they declare**. A reference resolves if it is a local
fragment or a member of that set; anything else is refused, including a URL that merely looks like an
identifier. Resolution is by membership, not by a URL-shaped test, so "may compose" never becomes "may
name any URL". An `$id` is read from the document rather than supplied by the caller, and a second
document declaring an existing `$id` is refused, so the definition a reference resolves to cannot
depend on insertion order.

**2. External manifests remain self-contained, and the asymmetry is enforced.** The plain
`ToolSchema::parse` and its `Deserialize` use an **empty** set, so a stored manifest cannot compose.
A connector with a shared schema must inline it. The cost is stated rather than hidden: a size cost at
authoring time, paid against a correctness property paid on every validation. The rule is asserted
from both sides — a deserialized schema with an unresolved reference must fail to load.

**3. Availability has two sources, and the declaration wins when it is restrictive.** The definition
declares what the capability is ("this build cannot do it"); the registry holds an optional runtime
state for now. `Availability::Unavailable` from either source makes the tool uncallable, so a health
check cannot enable something the build declared impossible. Clearing a runtime state restores the
declared value rather than having to remember it.

**4. Registration is per-tool but a manifest load is atomic.** `define` refuses a collision.
`define_all` registers a whole batch or none of it, and a collision **within** the batch is a refusal
rather than last-writer-wins. A partial connector load is rejected because it fails open: nineteen of
twenty tools available, no error reported, and a call that fails against a tool nothing registered.

**5. Model-facing discovery and operator-facing inventory are separate methods.** `discover` offers
only tools that can run, bounded, because offering a tool a model cannot call invites a failing call
for no benefit. `inventory` lists everything with its availability and reason, because "why is this
tool not offered" is the operator's question. They are not one method with a flag, so an operator view
cannot be handed to a model.

**6. A `ToolSummary` carries selection fields and no authorization fields.** `id`, `title`,
`description`, `source`, `effects`, and `risk` are present because a model choosing between two tools
benefits from knowing one sends mail and another reads a file. `required_scopes`, `approval`, and the
schemas are absent, because authorization is decided by policy from the registered definition and a
second copy here would be a second place for it to be wrong. A test asserts the serialized field list,
so the omission is checked rather than intended.

## Consequences

- `P3-001`'s restriction becomes precise instead of total: a JARVIS-authored schema may compose; an
  external one may not. Both are tested, including the control that an unsupplied reference is refused
  — without it, the permissive test would pass on a build that fetched the URL.
- A connector manifest cannot influence which document is loaded, because it cannot reference one at
  all. `$id` shadowing is impossible from external input and refused within a set.
- Availability can change without editing a definition, which is what makes health refresh
  representable. A runtime state can narrow but never widen the declared set.
- The registry cannot grow without bound, and model input size is independent of how many connectors a
  user installed — the bound that matters, since a user adding connectors must not silently reduce the
  context available for their actual question.
- A tool upgrade must change the version. Updating behaviour under an unchanged version would make a
  stored intent mean something it was not written against, which is the one thing the `version` field
  exists to prevent.
- Discovery is truncated, not unbounded, and the omissions are reported by count so a caller can tell
  "there are no more" from "there were more".

## Alternatives considered

- **Refuse every `$ref` permanently and require inlining.** Rejected as the end state, not as a
  principle: it forces duplication in JARVIS-authored schemas where the document set is closed over
  content this project wrote. Kept for external input, where it is the point.
- **Thread a document set through `Deserialize`.** Rejected: it gives a manifest the document-selection
  decision, and `$id` is document-controlled, so a manifest could shadow a JARVIS definition.
- **A URL allowlist instead of a document set.** Rejected: an allowlist has to be maintained and can be
  widened by mistake, whereas membership in a `BTreeMap` cannot be widened by any input.
- **Fetching with a size and scheme limit.** Rejected: a limit controls the damage of a fetch, not
  whether one happens, and nothing in a tool contract requires resolution over the network.
- **`bundle()` to inline referenced documents at registration.** Considered and not chosen: it mutates
  a schema's document shape (into `$defs`) and therefore what `document()` returns, and the two
  constructor split is clearer about which path may compose.
- **Deriving `ToolId` from a dotted string with `ToolId` holding the whole name.** Already rejected in
  ADR-0015; it resurfaces here only to note that the registry keys on the structural type, so two
  spellings of one identifier cannot both register.
- **Merging `discover` and `inventory` behind a flag.** Rejected: one wrong flag leaks unavailable
  third-party tool names into model input.
- **Per-tool registration only.** Rejected: it makes a partial connector load easy to write and
  invisible when it happens.
- **Last-writer-wins for a collision.** Rejected by `docs/architecture/tools-and-connectors.md`:
  "collisions are errors".

## References

- [docs/architecture/tools-and-connectors.md](../architecture/tools-and-connectors.md)
- [docs/architecture/repository-layout.md](../architecture/repository-layout.md)
- [docs/research/integrations/json-schema-validation.md](../research/integrations/json-schema-validation.md)
- [ADR-0015: Tool contract consistency and offline schemas](0015-tool-contract-consistency-and-offline-schemas.md)
- [ADR-0005: Canonical tools and MCP](0005-canonical-tools-and-mcp.md)
