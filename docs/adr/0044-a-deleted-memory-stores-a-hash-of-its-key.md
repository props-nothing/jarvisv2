# ADR-0044: Two columns the memory tables deliberately do not have

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-002` implements the memory store: `memories`, `entities`, `entity_aliases`, `entity_relations`,
`memory_entities`, and `memory_tombstones`. Three of the acceptance invariants in
`docs/quality/acceptance-tests.md` are *storage* properties rather than domain ones — deletion removes
the text, deletion blocks resurrection, and one workspace's memory does not enter another's context — so
the schema is where they are decided, and the shape of the schema is what this ADR records.

Writing the tables produced four decisions that are not obvious from reading them, and three were
resolved by finding that the obvious shape was wrong:

1. **A structured claim was stored as one JSON document.** It is a subject/predicate/object triple.
2. **`memory_tombstones` stored the deleted claim's search key.** The tombstone needs to recognise the
   claim on a re-ingest, and the search key is what a re-ingest would compute.
3. **"A relationship memory starts as a proposal" was written as a table `CHECK`.**
4. **`from_stored` could not read a deleted row.** The content rule ran before the stored status was known.

The first three are schema decisions and the fourth is a domain-constructor consequence of the same
reasoning, which is why they are recorded together.

## Decision

### 1. The structured claim is three columns, not one JSON document

```sql
claim_subject TEXT CHECK (claim_subject IS NULL OR length(claim_subject) BETWEEN 1 AND 256),
claim_predicate TEXT CHECK (claim_predicate IS NULL OR length(claim_predicate) BETWEEN 1 AND 256),
claim_object  TEXT CHECK (claim_object  IS NULL OR length(claim_object)  BETWEEN 1 AND 256),
CHECK ((claim_subject IS NULL AND claim_predicate IS NULL AND claim_object IS NULL)
    OR (claim_subject IS NOT NULL AND claim_predicate IS NOT NULL AND claim_object IS NOT NULL))
```

This is the repository's own rule from `docs/data/schema.md`: **"JSON stores versioned provider payload
fragments or flexible metadata, not core relationships that need constraints."** A claim triple is a core
relationship — the thing "does the user prefer X or Y" is a query over — so it gets columns. Three
consequences follow, and each is a reason rather than a benefit:

- **A per-part length bound is expressible.** One document can bound its total bytes, which is what the
  first cut did, and that permits a 8000-byte subject and an empty object.
- **"Both or neither" is unrepresentable rather than checked by a reader.** A claim with a subject and no
  predicate is not a claim, and the paired `CHECK` is the same shape the claim-triple rule uses elsewhere
  in this schema.
- **A lookup by predicate is an index seek.** `memories_claim_idx` is partial on `claim_predicate IS NOT
  NULL` because most memories carry no claim.

**Rejected: a JSON document with a generated-column index.** It would answer the query and keep one column
— at the cost of a generated column that is a second copy of the document, and a reader still unable to
constrain the parts.

### 2. The tombstone stores a hash of the search key, not the key

`memory_tombstones.search_key_hash` is `SHA256(search_key)`, and the column is deliberately named for what
it holds.

**The acceptance invariant is "deleting it removes text and derived indexes".** A search key is derived
text: `MemorySearchKey` is built from the claim's type, entities, and words, so a retained key *is* a
readable reconstruction of what was deleted — case-folded and word-sorted, but the words are the words. A
tombstone holding the key would satisfy "resurrection is blocked" while violating "the text is gone", and
the violation would be invisible because the tombstone is a different table that no retrieval reads.

A hash satisfies both. It recognises the same claim on a re-ingest and it holds nothing readable, and it is
the same technique `ADR-0018` and `P3-016` use for an intent digest.

### 3. "A relationship starts as a proposal" is not a table constraint

`MemoryType::Relationship::requires_confirmation()` makes a relationship memory start as `Proposed`, and
the domain enforces it at construction. It is **not** a `CHECK` on `memories`.

The reason is what a `CHECK` is: it applies to **every** write to the row, not to the row's insertion. A
constraint reading "a relationship row is never `active`" would make a *confirmed* relationship memory
unstorable — it would forbid the transition the confirmation exists to perform. The invariant is about
*creation*, and a column constraint cannot express "at insert" without also constraining every later
update.

This is the same distinction `ADR-0041` draws about a sandbox guarantee and `ADR-0017` about a policy
outcome: a rule enforced in the wrong place is not a stricter rule. Here the rule's home is the
constructor, and the schema states the parts of it that *are* per-write properties (a deleted row holds no
text, a claim triple is all-or-nothing, a model inference holds no confidence above `unverified`).

### 4. The stored status travels into the constructor, because deletion is the one status whose content is empty

`MemoryRecord::from_stored(parts, state)` takes a `StoredMemoryState`, and `state.status` is what decides
whether empty content is a tombstone or a corrupt blank row.

The first cut derived the record and then stamped the stored status on:

```text
MemoryRecord::from_stored(parts)   -> runs the content rule, which refuses empty content
  .from_stored_state(status, ..)   -> applied the status after the refusal had already happened
```

A deleted row is **required** by the schema to have empty content, so no deleted row could be read back at
all — `a_deleted_memory_is_not_read_back` failed with `InvalidMemoryRequest { field: "content" }`. The two
halves of one rule ("deleted ⇒ empty" and "not deleted ⇒ non-empty") were in two places that could not see
each other, so neither could be applied correctly.

Collapsing them into one constructor makes the rule expressible:

```text
build(parts, require_entities, permit_empty_content)   the one body, both parameters explicit
from_stored(parts, state) -> Result<Self, InvalidMemory>
```

and it adds the two checks that need both values in hand: empty content is permitted **only** in
`Deleted`, and a `Deleted` row that still carries text is `InvalidMemory::DeletedRetainsText` — the
decode-side counterpart of the schema's `CHECK`.

## Consequences

- **Two independent enforcers, on purpose.** The schema `CHECK`s and the constructor rules state the same
  trust/kind, model-inference, provider/preference, and deletion properties. The `CHECK`s cover rows this
  repository writes; the constructor covers a row written by another build, restored from a backup, or hand
  edited — the same argument `ADR-0044`'s neighbours in the approval and tool-call repositories make.
- **The entity-scoped read binds its parameter.** `read_entity_memories`'s statement takes a third
  parameter, and the shared runner binds it as an `Option` so a caller cannot supply the entity-scoped
  statement without its entity. **The first cut gave the runner an `entity_id` parameter and never bound
  it**: the query still ran, silently ignoring the filter, and only the compiler's arity check found it —
  which is the failure mode a shared runner is supposed to remove rather than introduce.
- **`delete()` clears `structured_claim` as well as `content`.** It already did, and the schema's own
  `CHECK` makes it non-optional: the claim is derived text, so a row that kept it could not be stored.
- **A decode now needs a `StoredMemoryState`**, which is a five-field copyable struct rather than five
  positional arguments — the fifth positional argument would be a `u32` next to a `UtcTimestamp`.

## Honest limits

- **Nothing retrieves or ranks a memory yet.** The reads exist (`read_workspace_memories`,
  `read_entity_memories`) and are ordered by recency only. Exact, full-text, importance, entity, and
  workspace ranking is `P4-004`; the embedding fields are `P4-005`, and no column for them exists here.
- **`record_memory`'s duplicate detection is exact.** It compares the search key, which normalizes case,
  whitespace, and word order only. A paraphrase is a second memory, deliberately: over-collapsing two
  different claims loses information the user gave, while a duplicate costs one row.
- **The entity links are written after the row, not in one transaction with it.** A failure between the two
  leaves a memory with fewer links rather than a link to a memory that does not exist — the safe direction,
  and not the same thing as atomic. `P4-008`'s deletion surface is where an atomic multi-table write will
  be needed, because a full user-deletion touches every table at once.
- **`entity_relations` has no cycle check.** A relation cannot name itself, and nothing forbids
  `a -> b -> a`. Whether a cycle is meaningful for a relationship graph is a question `P4-004`'s traversal
  has to answer, and answering it now would be guessing.
- **Falsified, one guard each.** Forcing `is_tombstoned` to return `false` made
  `deletion_removes_text_and_blocks_a_re_ingest` fail; removing the workspace scope from the read statement
  made `a_memory_cannot_be_read_from_another_workspace` fail. Both restored and re-run green.

## Alternatives considered

- **One `memories` table with a `deleted` flag.** Rejected: a flag keeps the text, which is precisely the
  invariant. Clearing the text in place and keeping the row is what makes the deletion observable without
  the text, and the tombstone is required because a cleared row cannot be matched by the unique index it
  no longer participates in.
- **A tombstone that references `memories(id)` with `ON DELETE CASCADE`.** Rejected: a full user-deletion
  removes the memory row, and the tombstone must outlive it. Writing the identifier *without* a foreign key
  lets an operator line the two up while keeping the tombstone effective.
- **Storing the deleted claim's search key in the tombstone and relying on access control.** Rejected: the
  retention requirement is about what is *stored*, not about who may read it, and a hashed key answers the
  same question with nothing to protect.
- **Making `from_stored` take the status as a fifth positional parameter.** Rejected: five positional
  arguments, two of them optional timestamps and one a counter, is a call site where two arguments can be
  transposed without a type error appearing — and the transposition would be a memory claiming the wrong
  retrieval history.
