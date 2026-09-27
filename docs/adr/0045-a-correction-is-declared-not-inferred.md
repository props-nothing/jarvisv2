# ADR-0045: A correction is declared, because a search key cannot detect one

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-003` implements candidate admission: the lifecycle `docs/architecture/memory-and-context.md` draws for
turning a model's proposal into a stored memory. Its rule set is stated in the pipeline itself — classify,
support, resolve entities, compare against the store, dedupe, propose review — and the comparison stage is
where the document's own requirement pulls in two directions at once:

> Never overwrite contradictory memory silently. Create a new claim, link `supersedes`, retain the
> correction trail according to policy, and stop retrieving obsolete claims as current truth.

So the comparison stage has to tell three situations apart: the workspace holds nothing (write a new
memory), it holds this same claim (reinforce rather than duplicate), or it holds a **different** claim that
this one replaces (write a correction and link `supersedes`).

The obvious implementation of the third case is an inference. `P4-001` and `P4-002` gave a memory a
`MemorySearchKey` so that two identical claims collide, and the natural reading is that two claims sharing
an *entity* but differing in content are the "contradictory memory" the document is about. The first cut
therefore compared on content:

```text
existing = the workspace's current memory whose key matches the candidate's
  content equal      -> Duplicate
  content different  -> Correction
```

**That branch is unreachable, and the reason is a property of the search key.** The key is built from the
memory type, the entity identifiers, and the content's words — sorted, deduplicated, case-folded:

```text
MemorySearchKey = "{type}|{entity_ids}|{sorted_distinct_words}"
```

So two records with the same key have the same *set* of words. A key lookup can therefore only ever return
a memory whose words match, and `content different` never happens. `normalized_equal` — the comparison the
pipeline used — was written to match the key's normalization exactly, which means it agreed with the key by
construction and could not disagree with it.

The failure was found by a test rather than by review, and the way it surfaced is worth recording: the
correction test asserted a `Correction` outcome and got `Duplicate`. The natural repair — "make the fixture
differ more" — would have been wrong. The fixture was already as different as a shared key permits, which
is not different at all.

The next candidate, inferring a correction from *"the same entity, different words"*, is worse and was
rejected before implementation: two unrelated facts about one person ("Alice is my sister", "Alice lives in
Rotterdam") share an entity and differ in words, so every second fact about anyone would retire the first.

## Decision

**A correction is declared by the candidate, not inferred by the pipeline.**

```text
MemoryCandidate { ..., supersedes: Option<MemoryId> }
```

The field is set by whoever decided the new claim replaces the old one — the user ("I prefer tea now"), or
the component that read a later statement as being about the same subject area. `admit` checks it **before**
the comparison against the store, because it is an assertion about the candidate's own history rather than
an observation about the store, and it outranks the fact-or-proposal decision: a corrected *relationship*
claim must supersede rather than become a proposal, or the obsolete claim stays current truth.

`MemoryCandidateComparison::Correction` remains, with a narrowed meaning: it is returned when `existing`
holds different words, which a correct caller cannot produce because its own key lookup would not have
returned that row. It is kept because a caller **supplies** `existing`, and a caller that passed a row from
a wrong lookup would otherwise have its row reinforced as though it held the candidate's claim. So the
branch is the symptom of a key-mismatch bug, and returning `Correction` rather than `Duplicate` is what
makes that bug produce a visible supersession instead of a silent merge. `compare`'s doc comment states
this rather than leaving the branch looking reachable.

**The refusal vocabulary gained a distinction the same reasoning produced.** `MemorySearchKey` bounds each
key word at `MAX_SEARCH_KEY_WORD_CHARS` (64) as well as the whole content at `MAX_MEMORY_CONTENT_CHARS`
(4096), so a 4096-character single word is within the *content* bound and unkeyable. The first cut reported
`CandidateRefusal::Content` for both, which would send an operator shortening text that already fits. There
is now a separate `Unkeyable`, and its test asserts both bounds, because they are properties of the same
pair of constants and a single-word fixture would conflate them.

## Consequences

- **Supersession requires an explicit act.** A caller that wants a correction must say so. That is the
  honest shape: only the party that read the later claim knows the two are about the same subject area, and
  a pipeline that guessed would either miss corrections or invent them. The cost is a field a caller must
  remember to set; the alternative cost was retiring a memory for every unrelated fact about a person.
- **The reinforcement signal stays correct.** A duplicate reinforces, an `AlreadySuperseded` claim is left
  alone, and a correction links `supersedes`. `P4-004` ranks by prior useful retrieval, so counting a
  retrieval for a claim the candidate did not repeat would corrupt the signal it ranks by.
- **`compare`'s `Correction` branch is a diagnostic, not a path.** Its doc comment says so, and the
  consequence is that a key-lookup bug surfaces as a supersession rather than as a silent merge — visible
  in the `supersedes` link rather than in a wrong retrieval count.
- **The ordering is asserted, not assumed.** The tombstone is checked before the duplicate because a
  deleted row has no search key and so cannot appear as `existing`; the declared correction is checked
  before the proposal branch; the entity set is required before the key is derived from it, so the
  extractor's proposals are never a fallback. Each ordering has a test whose fixture makes the wrong order
  observably wrong — the tombstone test's fixture is *both* tombstoned and matching, since otherwise either
  order passes.
- **Confidence is lowered, never raised.** A model inference is admitted at `Unverified` and as a proposal
  regardless of what it claimed; a document claim is capped at `Likely`; a caller claiming *less* certainty
  than its source could support is not overruled. The invariant "an inferred preference never appears as
  confirmed fact" is a property of the stored value rather than a check a later reader must remember.
- **Sensitivity is a maximum of three floors.** The type's floor (a relationship claim is `Confidential`),
  the content's (a credential or health term raises it to `Restricted`), and the extractor's — which is
  used when it is at least both. Only one direction is safe to correct automatically, because exclusion
  from a remote model is decided from the *stored* level.

## Honest limits

- **Nothing extracts a candidate from anything yet.** `admit` takes a candidate that already exists. The
  read-from-conversation half is `P4-007`, where the context assembly and the provider boundary live, and
  the model-facing extraction prompt has not been designed.
- **Nothing writes an admission.** The pipeline decides and returns values; persisting them is the caller's
  act, and no caller exists. So the end-to-end property — a model's proposal becoming a stored proposal and
  reading back with the status the pipeline intended — is **unproven**, and proving it is `P4-004`'s or a
  composition slice's.
- **Nothing reads the store.** The four store facts arrive as borrowed values in `CandidateContext`, because
  entity resolution needs the repository in `jarvis-storage` and `jarvis-core` has no storage dependency. So
  the caller owns the queries, and a caller that looked a key up incorrectly is exactly the case
  `compare`'s `Correction` branch is left to catch.
- **The restricted-content check is a keyword list, not a classifier.** It catches the shapes a claim's text
  can carry — eight credential prefixes and ten terms naming credentials or health data. A secret that looks
  like ordinary prose is caught by nothing here. This is recorded rather than papered over: the check can
  only ever *raise* the level, so a miss leaves the caller's own classification in place.
- **Supersession is a single link, not a chain walk.** `supersedes` names one memory, and nothing here
  validates that the named memory exists in the candidate's workspace — that is the store's foreign key and
  `P4-002`'s `apply_memory_transition`, which is where a cross-workspace identifier is refused.
- **A candidate carries no identifier**, so the domain's `SupersedesSelf` refusal has nothing to compare at
  this stage. A memory's identifier is assigned when it is stored, so the value that could name itself does
  not exist yet; the check belongs where the identifier does, and it is `P4-001`'s.

## Alternatives considered

- **Infer a correction from "same entity, different content".** Rejected as unbuildable with this key, and
  then rejected on its own terms: two unrelated facts about one person share an entity and differ in words,
  so it retires a memory for every second fact about anyone. It also cannot be tested — a fixture that
  produces a `Correction` from a key lookup is a fixture that demonstrates the key lookup is wrong.
- **Widen the search key so it can distinguish claims that share a subject.** Rejected: the key exists to
  make identical claims collide, and `P4-001` records the reason it normalizes case, whitespace, and word
  order *only* — over-collapsing loses information the user gave, and under-collapsing a duplicate costs one
  row. Widening it to encode a claim's identity would make the same claim produce two rows across a
  formatting change, which is the duplicate it exists to prevent.
- **Have `admit` return a `Result` whose error carries the admission.** Rejected: the three comparison
  outcomes each call for a **write of a different kind**, so they are admissions, not failures. Returning
  them as errors would mean a caller branching on the error's payload to decide what to write, which is the
  shape where the write and its justification drift apart.
- **Let the pipeline look the key up itself.** Rejected: it would give `jarvis-core` a dependency on
  `jarvis-storage` and put a store read inside a domain crate. `P4-002` already establishes that the reverse
  arrow is the direction that exists.
- **Report `Content` for an unkeyable claim.** Rejected: the two are different rules and are fixed
  differently, so one refusal would send an operator shortening text that is already within the bound. This
  is the same reasoning `P4-001` records for its own twenty error variants.
