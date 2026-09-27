# ADR-0048: Semantic similarity is one conditional signal, and an incomparable vector is refused

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-006` adds hybrid retrieval: `docs/architecture/memory-and-context.md` lists nine ranking signals, and
`P4-004` implemented eight of them. The ninth — semantic similarity — was left out deliberately, and
`ADR-0046` recorded why: with no embedding there was no data to produce it from, and a row that could only
ever be zero is the weights table claiming a signal it cannot compute.

`P4-005` built the embedding port. The signal is now computable, which raises four questions the previous
slice did not have to answer:

1. **Where do the vectors come from?** They are not in the `MemoryRecord` and not in the `MemoryQuery`, and
   `jarvis-core` cannot depend on `jarvis-models` — the dependency runs the other way, so the domain cannot
   name an embedding type.
2. **What happens when they are missing, which is the common case?** A memory stored before the embedding
   model existed has no vector, and a deployment with no embedding provider configured has none either.
3. **What happens when they are incomparable?** A query embedded by one model and a memory embedded by
   another are both valid vectors and cannot be compared.
4. **What happens to the weights?** Adding a ninth signal to a table that must sum to a constant is an
   arithmetic change, not an insertion.

The first three are one decision each, and the fourth produced the most interesting finding.

## Decision

### 1. The query's vector and the memory lookup are one optional value

They arrive as a [`ScoringContext`], and `jarvis-core` defines its own borrowed `SemanticVector` carrying
provider, model, version, normalization, and the values. `jarvis-models`' `Embedding` cannot cross the
boundary — `jarvis-models` depends on `jarvis-core`, so the domain naming that type would be a cycle — and
the domain does not need it: the four identity fields and a `&[f32]` are the whole contract.

They are **one** value rather than two arguments because they must agree. Passing the query vector
separately from the memory lookup would make "a query vector with no memory vectors" and "memory vectors
with no query vector" both representable, and both are meaningless.

### 2. No index is a zero score with a **recorded reason**, not an absent row

When the context carries no index the signal is `0` and the absence is reported as `SemanticAbsence::NoIndex`.
The distinction matters and is the reason `SemanticAbsence` exists at all: a caller must be able to tell

- apart from a zero because the provider was never configured,
- apart from a zero because the memory has no vector,
- apart from a zero because the vectors are incomparable, and
- apart from a genuine zero — two orthogonal directions.

These are four different operational answers to "why did semantic similarity not help here", and a bare `0`
collapses them. It also preserves `ADR-0046`'s principle in a form that now applies: the table lists a signal
that **is** computable, and the *reason* a particular call produced nothing is data rather than silence.

### 3. An incomparable vector scores zero, and the field that differs is named

Provider, model, and version are compared before any arithmetic, and a mismatch produces
`SemanticAbsence::Incomparable("model")` — the field, not a boolean. The vectors' own lengths are compared
too, which happens to make a declared dimension field unnecessary: a length that contradicted a declared
dimension would be the defect `P4-005`'s constructor already refuses, and repeating the field here would let
a caller build a pair this module believes and `jarvis-models` would refuse.

**This is the guard the whole feature turns on.** A cosine over two vectors from different models is a number
with no meaning that still lands in the usual range, so a missing check does not look like a bug — it looks
like a slightly worse ranking. That is the defect class that survives review.

A **zero-magnitude** vector is refused for a related reason: the cosine is undefined, and a `NaN` reaching a
total would poison a whole ranking rather than one signal.

### 4. A negative cosine scores zero, and an orthogonal one scores zero **for a recorded reason**

A component score is a magnitude out of `SIGNAL_SCALE`. Letting an opposed vector pull a total *down* is
something no other signal can do, and the weights are not shaped for it, so `cosine <= 0.0` scores zero.

The reason is recorded as `NoSimilarity` rather than as "no absence", and that was a finding rather than a
choice: the first implementation returned a zero with `absence: None` for the whole `cosine <= 0.0` case, and
a test asserting that an orthogonal pair scores `SIGNAL_SCALE / 2` revealed it. Two separate mistakes were in
that one line:

- The test's expectation was wrong — an orthogonal pair has cosine `0.0`, not `0.5`. The **code** was right.
- The **code** still conflated two cases. An orthogonal pair and an opposed pair both produce zero, but so do
  a missing vector, an incomparable pair, and a zero magnitude. A stored explanation that cannot distinguish
  real arithmetic from a refusal is the same defect `ADR-0046` refused when it made the total *be* the sum of
  the contributions.

### 5. The weights were rebalanced, not renumbered

Nine weights must still sum to `TOTAL_WEIGHT`, so `P4-004`'s eight values could not simply gain a ninth. The
new table keeps every stated ordering from the document and keeps the cap that no weight exceeds a quarter of
the total:

| Signal | `P4-004` | `P4-006` |
| --- | --- | --- |
| exact identifier | 250 | 250 |
| keyword | 200 | 175 |
| **semantic** | — | **150** |
| entity overlap | 150 | 125 |
| recency | 125 | 100 |
| temporal | 100 | 75 |
| importance | 75 | 60 |
| source reliability | 60 | 40 |
| reinforcement | 40 | 25 |

Two properties are asserted rather than asserted-in-prose: the sum is `1000`, and no weight exceeds `250`.
The `dominate-by-accident` comparison from `ADR-0046` still holds, because the cap is unchanged and a
generalist still beats a specialist.

### 6. The reason's precedence is not the weight's order

The document lists semantic similarity **above** entity overlap, and the weights keep that. The *reason* a
user is told puts semantic similarity **last** among the four matching signals.

These answer two different questions, and collapsing them would make one of them wrong. A weight says how
much a signal contributes; a reason says how confidently the result can be described. "This memory means
something close to what you asked" is an inference, while "this memory is about the entity you named" and
"this memory repeats your words" are present in the text — so a memory whose words match is reported as a
keyword match even when its meaning matches too.

## Consequences

- **The signal is present, and its absence is explainable.** The nine-signal table is complete against the
  document for the first time, and every zero has a stated cause.
- **`SIGNAL_COUNT` replaces the array literals' `8`s.** The weights, signals, and contributions arrays now
  share one constant, so a signal cannot be added to the struct and forgotten in three other places — the
  transposition hazard the named struct already guarded against is now guarded in the array forms too.
- **A memory without an embedding is not a worse memory.** It scores zero on one of nine signals and is
  ranked on the other eight, which is the document's "embeddings are one signal" as arithmetic rather than as
  policy.
- **The scoring is still pure and still integer at the total.** The only floating-point arithmetic is inside
  one signal, and it is rounded to an integer score once at the end — so a stored score remains reproducible
  and an inclusion reason remains verifiable.
- **A caller that wants no embeddings writes one word.** `ScoringContext::without_semantics()`, which exists
  because a `Default` would read as "unspecified" and quietly disable a signal.

## Honest limits

- **Nothing produces the vectors.** The port from `P4-005` is not wired to anything: no daemon route embeds a
  memory, no indexing job populates a lookup, and nothing writes a vector to storage. The tests build vectors
  by hand, so what is proven is the scoring and the guards, not an end-to-end retrieval with embeddings.
- **Nothing calls `rank` either.** No context assembly, no CLI verb, no route — the module is still a
  complete, tested, uninvoked component, and `P4-007` is the integration.
- **The weights are reasoned, not calibrated.** Moving eight values to make room for a ninth is an educated
  rebalance; nobody has looked at a real result set and asked whether semantic similarity should outweigh
  entity overlap. `P4-004` recorded the same limit, and this slice makes it slightly larger by changing the
  numbers rather than adding to them.
- **The lookup is a closure, so the caller decides what "the index" is.** That is the seam a real vector
  index will plug into, but it also means the module cannot say anything about how a vector is found —
  brute-force scan, pgvector, or anything else. `P4-009` owns that.
- **The cosine is computed in `f32`.** Two platforms could disagree in the last bit of a score, which for a
  rounding boundary could change a total by one. The stored score is an integer so it stays reproducible on
  the platform that computed it, but cross-platform bit-identity is not claimed.
- **Relationship overlap and active project/task relevance remain absent**, unchanged from `P4-004`: there is
  no relation traversal and no project or task in the record.
- **No calibration against a query with no text.** A query with no text and no entities produces a semantic
  term and nothing else, so its ranking is driven entirely by meaning. That is arguably right for such a
  query and it is untested, because no caller constructs one yet.

## Alternatives considered

- **Put the embedding identity fields on `MemoryRecord`.** Rejected: it makes every memory carry embedding
  provenance whether or not it has one, and it puts a provider's model name in the canonical domain record —
  which `AGENTS.md` refuses.
- **A `&dyn Fn` lookup instead of an owned box.** Rejected after it was written: every call site passing a
  closure literal failed to compile, because a temporary closure borrowed into a longer-lived context is
  dropped at the end of the statement. Owning one box makes the cost per ranking rather than per memory, and
  the natural way to write a lookup works.
- **A `ScoringContext::default()` that disables semantics.** Rejected: the same reason `P4-004` refused
  permissive query defaults — a default is what a caller who did not think about the field gets, and here the
  consequence is a silently missing signal.
- **Treating a missing vector as a low score rather than a reported absence.** Rejected: it states "this
  memory is semantically unlike the question" about a memory whose content was never compared.
- **Returning a negative score for an opposed vector.** Rejected: no other signal can reduce a total, and the
  weights are not shaped for a signal with a negative range.
- **Skipping the length check and letting the dot product truncate to the shorter vector.** Rejected: it is
  the silent wrong answer a comparability check exists to prevent, and it produces a plausible number rather
  than an error.
- **Comparing only the model, not the provider and version.** Rejected: two providers can serve the same
  model name from different weights, and a provider can update a model without renaming it. The cheapest
  check is the one that catches the case nobody modelled.
- **Making the semantic signal conditional in the weights table** — present only when an index exists.
  Rejected: the table is a constant, a per-call table would make two totals incomparable, and the signal *is*
  implemented, so the row belongs.
- **Letting the semantic weight take the room from `keyword` only.** Rejected: no single signal should absorb
  the whole cost of a new one, and spreading it keeps every relative ordering the document states.
