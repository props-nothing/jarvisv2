# ADR-0052: An entity is read from the store, never invented from a request

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-010` requires the Phase 4 acceptance cases to pass: **A08** (memory lifecycle —
`docs/quality/acceptance-tests.md`) and **A09** (workspace isolation). Both are process-level claims about an
installed `jarvisd`, so the work was to write a gate that runs the real daemon and drives it over its
published HTTP API, the way the three phase gates before it do.

Writing that gate found three defects, and the first two are the kind that only a process-level test can
find: the library suites were green, and each was a path no unit test had taken.

1. **The remember path invented any entity the caller named.** `resolve_entities` only *parsed* the
   identifiers, and the link step then called `record_entity` — so a `POST /api/v1/memories` naming a subject
   that did not exist **created that subject** and answered `201`. The gate found it because its A08 case
   needed a subject and used a fresh identifier for a negative check.
2. **A claim could name another workspace's entity.** Nothing compared the entity's workspace against the
   caller's, so a caller could write a claim into its own workspace pointing at another workspace's subject.
3. **`P4-010` needed a second workspace and nothing could create one.** Every profile has exactly one, seeded
   by migration `0005`, and `A09`'s "two workspaces" cannot be tested without another.

## Decision

### 1. An entity identifier is a reference the store must already know, and an unknown one is a refusal

`resolve_entities` now **reads** the entity and refuses it unless all three of these hold:

- it exists, so a typo produces a named refusal rather than a claim about a fabricated subject;
- it belongs to **this** workspace, so a caller cannot attach a claim to a subject across a boundary every
  read respects;
- it is **usable** — not merged and not deleted — because a merged entity's claims belong to the winner and a
  deleted one's to nobody, so a new claim against either would attach itself to a name that no longer denotes
  anything. `StoredEntity::is_usable` is the domain's own predicate rather than a comparison restated here.

Two consequences are worth stating plainly, because the defect was not merely a missing check.

**A fabricated entity is a memory that cannot be found or corrected.** `ADR-0050` refused the *placeholder
subject* — filing a claim against a stand-in when the caller named nothing — on exactly this reasoning: a
claim attached to a placeholder *looks* resolved, so a later question about the real person will not find it
and nothing in the store says why. The placeholder *mechanism* stayed in place one layer down, and the
acceptance gate is what surfaced the inconsistency.

**The identity vocabulary was caller-controlled.** `docs/architecture/identity-and-workspaces.md` requires an
entity to be established through resolution: verified provider IDs, exact identifiers, user confirmation, or a
probabilistic match recorded as such. A caller asserting an entity over the wire is none of those. So the
wire surface now means "the subjects this claim is about", which is what the field name says, and it cannot
be used to introduce a subject.

The refusal is `422` naming `entity_ids` and echoing the offending identifier, because the caller is the party
that can fix it. A missing entity is **not** reported as a storage failure: the request named something the
workspace does not have, and `Storage` would send an operator to check a database that is working.

### 2. The entity set is read, never written, on the memory write path

`apps/jarvisd` no longer calls `record_entity`. The `memory_entities` rows are written by `record_memory`,
which inserts them in the same call as the memory row — so a claim cannot exist without the subjects it named,
and the write path has no second place where an entity can appear.

### 3. `record_workspace` exists, and it is deliberately not a multi-tenant feature

A09's claim is about two workspaces, so `jarvis_storage::record_workspace` was added. It records a row and
nothing else:

- **no session**, no credential, no invitation, no membership;
- **no route**, and no actor can name a workspace — `apps/jarvisd` still resolves its workspace from the
  seeded identity, and `deny_unknown_fields` refuses a `workspace_id` in a body (`ADR-0050`);
- its own tests assert the two properties that make it safe to have: a recorded workspace is **distinct** from
  the seeded one, and recording one **does not change** the identity the profile resolves to.

That last assertion is the guard that keeps this from becoming a scope change by accident. If
`load_local_identity` ever returned a newly recorded workspace, every read in the product would silently
change scope, and only a test that recorded a workspace *and then re-resolved the identity* would notice.

The workspace's `mode` and `data_policy` are Rust enums mirroring the schema's `CHECK` values, asserted
literally against the schema's own strings — `local-only` uses a **hyphen**, which is the kind of value a
`String` gets wrong in one of two places.

### 4. The gate measures what exists and names what does not

The gate is a process-level test per acceptance case, on its own port and its own disposable profile, because
cargo runs integration tests in parallel and two daemons contending for one port produces the intermittent
failure that gets a gate disabled.

Three parts of A08 and A09 are unreachable through any product surface, and the gate **says so** rather than
asserting something the platform does not do:

- **Nothing creates an entity over the API.** The gate uses the storage crate's own `record_entity` — the same
  function `apps/jarvisd`'s unit tests use — to build A08's precondition, and then **asserts the refusal** for
  an unknown entity so the gap is measured rather than hidden. A user of the shipped product cannot record a
  memory, because they have no way to name a subject. That is the largest gap this slice exposes.
- **Embeddings are not written and there is no `memory_embeddings` table**, so A08's "embeddings, indexes,
  caches" half is asserted only for the parts that exist: the canonical text, the derived search key, the
  entity links, the supersession link, and the tombstone. `P4-009` owns the rest.
- **The model's context is not asserted.** A08 says JARVIS "retrieves" the preference; the gate asserts
  retrieval through the memory read and search surface, which is the document's user-facing retrieval.
  `P4-007`'s prompt path needs a configured executor model and a provider, and it is covered by the daemon's
  own tests rather than by a process gate that would need a model to run.

### 5. Each refusal is paired with an independent second fact

A gate that asserts only a status code can pass for the wrong reason. Every negative assertion here is paired
with a second one that a different mechanism would have to satisfy:

- the unknown-entity refusal (`422`) is paired with a store read proving the entity was **not created**;
- the cross-workspace entity refusal (`422`) is paired with a search proving no claim **matched** the refused
  words;
- the cross-workspace *read* refusal (`404`) is paired with a **positive** read of this workspace's own claim,
  so "refuses everything" cannot pass.

The search assertions use `is_a_match` rather than an empty result, because `ADR-0046`'s ranking legitimately
includes a recent claim that matched nothing. The gate's first version asserted an empty result and failed
against a correct implementation — the ranking's own distinction is what a test of "did this get stored" has
to ask about.

## Consequences

- **The wire shape is held still by the gate.** The daemon's replies are read as raw JSON rather than through
  the product's DTOs, because a client built from those types would follow a field rename without noticing.
  The gate's first run failed on `superseded["reference"]["status"]` being null — `MemoryDetailReply`
  flattens its reference, so the fields are top-level. A client written against the old nesting would have
  broken in production, and the gate caught it.
- **Falsified, one guard each.** With the entity-workspace comparison disabled, the gate stored a claim naming
  another workspace's entity and answered `201`. With the memory scope comparison disabled, a read of another
  workspace's claim answered `200` with its content. Both restored, green.
- **The acceptance cases are now covered by a gate that runs the product**, so a change to the memory
  lifecycle or to workspace scoping fails a test that exercises the daemon rather than a library.
- **Recorded limits.** No entity creation surface, so a remember is not reachable by a user of the product. No
  multi-tenant surface: `record_workspace` is a repository function with no route, and its own doc says so. No
  `memory_embeddings` table. The gate's `A08` case does not exercise the model's context, and `A09` does not
  cover "context build", a model call, a tool, a trace, or diagnostics — those surfaces either take no
  workspace or do not exist yet, and each is recorded rather than implied.
