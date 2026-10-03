# ADR-0126: An entity is its own surface, and identity is not trust

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P4-016`

## Context

`docs/architecture/memory-and-context.md` requires that every claim name a subject, and `docs/api/contracts.md`
lists entities among the surfaces a client needs. The shipped product had neither: `P4-008` records the
consequence in one line — "**No entity-creation surface exists**, so a remember is still unreachable by a user of
the shipped product" — and `P4-014` and `P4-015` each recorded the same limit from their own direction, because a
model's memory proposal and a session summary both need an entity identifier nobody could obtain.

The measurement behind this slice: **seven** entity-repository functions had no production caller at all —
`merge_entities`, `record_alias`, `resolve_alias`, `read_alias_candidates`, `read_entity_memories`,
`record_relation`, `read_subject_relations`. The entity machinery was written, tested, and reachable by nothing,
including the two functions that are exactly the "entity resolution" the recorded limit names as missing.

One further fact shaped the decision. While wiring the alias write, the schema's rule and the nearest domain
predicate turned out to be **different sets**:

- `0009`'s `CHECK` permits a `confirmed` alias only from `source_kind IN ('user_statement', 'user_correction')`.
- `MemorySourceKind::permitted_trust()` returns `Authoritative` for those **and** for `provider_record`.

## Decision

**Entities get their own surface — routes under `/api/v1/entities` and a `jarvis entity` verb group — rather than
being a sub-resource of memories.**

An entity is not a memory: different lifecycle (created, aliased, merged versus proposed, confirmed, corrected,
forgotten), different owner (the operator's vocabulary, not the model's), and a different role (it is what a claim
is *about*). Making it a sub-resource would have put "create the subject" behind the verb that requires one, which
is the ordering problem the recorded limit describes.

**A lookup returns every candidate with its evidence, never one row.**

`resolve_alias` documents the rule — "**Ambiguous aliases remain separate candidates**" — because a resolution
answering with one entity would turn a guess into an identity, and every later claim would inherit it. So
`GET /entities/lookup` answers with a list of matches, each carrying the aliases that matched it and a `verified`
flag, and the CLI's renderer prints them all with a disambiguation line when there is more than one. **The last
step is where this rule could be undone**, which is why the renderer has a test and why the daemon exposes one
route for both kinds of lookup rather than two that could diverge.

**The verdict is the conjunction over the aliases that matched one entity.**

An entity holding a verified email *and* a probabilistic one for the same value reports `verified: false`. The
question a caller is asking is whether this name denotes this entity, and one of the matching names is a guess.

**`MemorySourceKind::is_user_stated()` is added to `jarvis-core`, and it is deliberately not
`permitted_trust() == Authoritative`.**

Trust asks "may this content instruct"; identity asks "who established this". A `ProviderRecord` is authoritative
for the first and **cannot** establish an identity by itself. The two predicates were one line apart from being
treated as one, and the failure mode is specific: a guard written as the trust comparison accepts `provider_record`
where the schema refuses it, so the caller's error surfaces as a constraint failure — a `503` telling an operator
the database is unavailable, for a request they can fix. This was **falsified**: substituting the trust predicate
into the guard fails the route test with exactly `503` where `422` is required.

**The alias's verification and its source kind are both request fields, and their cross-field rules are checked in
the service as well as by the schema.**

The schema's rule is a relationship between two values the caller supplies, so neither can be derived without
making the rule unstateable. Checking it in the service is what makes the refusal name the **rule** rather than a
table, and the schema remains the backstop — the same two-layer split `0009` already documents for the memory
tables.

**A merge answers with the winner, and the direction is spelled `--into`.**

The loser is marked `merged` and points at the winner rather than being deleted, so a merge is auditable and
reversible — the rule the storage layer already implemented. The reply is the winner because that is the entity
that still denotes something, and each reply carries `linked_memories` because an operator merging duplicates
needs to know which side is used: a merge moves no links, so the wrong direction leaves every claim attached to a
retired name.

## Consequences

- **The recorded gap is closed**, verified live: `jarvis entity create` then `jarvis memory remember --entity <id>`
  against a real daemon reaches `status active`. That path was impossible before this slice.
- **Three rules that had no production caller now have one** beyond the seven above: `MemoryType::is_durable`'s
  sibling rules in the entity tree, `StoredEntity::is_usable` (used by the merge's own checks and the listing's
  filter), and the partial unique index on verified aliases (reached by the surface, refused as `422` with a
  remedy).
- **One new storage reads and two counters** exist that did not: `read_workspace_entities`,
  `read_entities_by_label`, `read_entity_aliases`, `count_entity_memory_links`. The last is the **entity-side**
  counterpart of `count_memory_entity_links`, and the distinction is the question: "what is this claim about"
  versus "how many claims are about this".
- **Two parsers became one.** `parse_summary_limit` was generalized to `parse_limit(raw, default)`, because there
  are now three listing bounds (200, 128, 50) and three parsers would be three places for a bound to be wrong.
- **Recorded as a limit:** a label lookup is **exact** (case-insensitive, whole-label), not a text search. A
  fuzzy match would turn a guess into an identity — the same rule the alias path follows — so a caller wanting
  partial names has no surface yet, and none is specified in `TODO.md`.
- **Recorded as a limit:** no entity **deletion**. `merge_entities` is the only way to retire one, and an entity
  created in error stays listed until it is merged into another. The architecture's entity lifecycle names
  archived and deleted states, and neither has an operation.
- **Recorded as a limit:** `entity_relations` remains unreachable (`record_relation`, `read_subject_relations`).
  A relation is a claim *between* entities and needs a surface of its own; it is not part of "create and find the
  subject a memory is about", which is what this slice names.
