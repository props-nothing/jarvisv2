# ADR-0051: The query is shaped by the index, and a vector the index cannot hold is refused

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-009` requires "PostgreSQL plus pgvector backend parity for the completed memory behavior", following
`ADR-0003` (SQLite local, PostgreSQL plus pgvector server) and `docs/architecture/storage.md`'s
"Store embedding provider/model, dimensions, normalization, input hash, chunker version, and created time.
Never compare vectors with incompatible metadata."

`P4-005` through `P4-008` built the embedding port, the semantic signal, retrieval into the prompt, and the
memory surface — and each recorded the same limit: **nothing writes an embedding to storage, and there is no
`memory_embeddings` table**. So this slice begins a vector index from nothing.

The external contract was researched first, as `docs/development/external-research.md` requires, and recorded
in `docs/research/integrations/postgres-pgvector.md`. Four findings there decide everything below.

1. **SQLx 0.9.0 has no `vector` type mapping.** The Postgres driver's type table lists `bool`, the integers,
   `f32`/`f64`, strings, `BYTEA`, `UUID`, `JSON`/`JSONB`, `TIMESTAMPTZ` and others, and no vector type of any
   kind. The value must cross the boundary as a **text literal**, so a hand-written codec is unavoidable.
2. **pgvector indexes a `vector` column only up to 2,000 dimensions** (its README states this for both HNSW
   and IVFFlat), while the *storage* limit is 16,000.
3. **The query shape decides whether the index is used at all.** "The `ORDER BY` must be the result of a
   distance operator (not an expression) in ascending order", and the README's counter-example is
   `ORDER BY 1 - (embedding <=> q) DESC` — which is exactly how a caller naturally expresses cosine
   *similarity*.
4. **`<#>` is the negative inner product**, "since Postgres only supports `ASC` order index scans on
   operators".

## Decision

### 1. The codec is a text form, and it is one module with no database in it

`crates/jarvis-storage/src/pgvector.rs` encodes `&[f32]` into pgvector's documented form and decodes it back.
Because SQLx cannot bind a vector, this is the only part of the pgvector path that can be wrong without a
server noticing — so it is the part that a machine with no PostgreSQL can falsify completely.

**The round trip is asserted by bit pattern, not by value.** `-0.0 == 0.0` is `true`, so a value comparison
cannot see a sign lost by the text form, and `1.0 / 3.0` and `f32::MIN_POSITIVE` are the cases a
fixed-decimal formatter fails. `f32`'s `Display` writes the shortest decimal string that round-trips to the
same `f32`, which is the property the assertion pins.

**The documented form is asserted literally, in addition to the round trip.** `encode(&[1.0, 2.0, 3.0])` is
`"[1,2,3]"`. A round trip alone cannot distinguish a correct form from a self-consistent wrong one, and a
wrong form is rejected by the server at insert — so a round-trip-only test would be green on code that never
works against a real database.

### 2. The codec crosses an adapter boundary as plain data, and that is the correct narrow waist

The codec takes `&[f32]` and returns `Vec<f32>` rather than `jarvis_models::EmbeddingVector`. That is forced
by `docs/architecture/repository-layout.md`'s dependency graph — adapters may depend on `jarvis-core` and
`jarvis-protocol` and **not on each other**, and `jarvis-storage` and `jarvis-models` are both adapters. The
same rule `ADR-0047` applied when it kept the embedding port's types inside the provider boundary.

The composition root, which may see both crates, maps between them. The consequence is recorded: this module
cannot ask an `Embedding` for its dimensions, so it works in `usize` and its refusals carry a count rather
than a domain type.

### 3. An embedding above the indexed cap is refused by name, not stored

`MAX_INDEXED_DIMENSIONS` is 2,000, and a vector above it is `PgVectorError::DimensionsTooLarge`, whose message
names the documented alternatives (`halfvec` to 4,000, binary quantization, subvector indexing,
dimensionality reduction).

The alternative — store it and let the index silently not apply — would make "we use pgvector" true while
retrieval fell back to an exact scan over every row. That is a performance claim with no evidence behind it,
which `AGENTS.md` forbids more strongly than it forbids a missing feature. **A refusal the caller can read is
worth more than a feature that is present and not doing what its name says.**

The domain's `EmbeddingDimensions::MAX` is 16,384, eight times this cap. The gap is a recorded fact and a
defect in neither crate: the domain bounds what a *provider* may return, and this bounds what pgvector can
*search*. Today no researched model exceeds 3,072 dimensions, so the cap is not yet binding — and the refusal
exists so that the day one does, the failure is a named error rather than a slow query.

### 4. The finite-component guard is on **both** sides, because `NaN` encodes successfully

`"NaN".parse::<f32>()` succeeds and `f32::Display` writes `NaN`, so a guard only on the read would let this
platform **write a row it cannot read**, and the write would look like it worked. Falsified: with the
encode-side check removed, `encode(&[1.0, f32::NAN])` returns `Ok("[1,NaN]")` and the test fails.

The read side check is separate and has its own reason: a `NaN` admitted into a comparison makes every
distance `NaN`, so the row would quietly rank last rather than reporting anything.

### 5. The metric is named, and the index class travels with it

`DistanceMetric` carries the operator (`<=>`, `<->`, `<#>`) *and* the operator class (`vector_cosine_ops`,
`vector_l2_ops`, `vector_ip_ops`) together, because the README requires "an index for each distance function
you want to use". A query using a metric whose index does not exist is an exact scan that looks like a working
search, so the two must not be independently choosable.

Naming them also makes the correct query shape the only shape this module offers. A caller writing SQL by hand
would reach for `ORDER BY 1 - (embedding <=> q) DESC` to get a *similarity*, which is the README's own
documented example of a query that uses **no index**.

### 6. Cosine is the default metric, and the distance-to-similarity conversion is a method

`docs/research/integrations/embeddings.md` records that the researched provider normalizes its vectors and
recommends cosine. Cosine is also the metric that gives comparable results whether or not a future provider
normalizes, which a dot product does not — and `Normalization::Unknown`'s own doc already says why assuming
normalization is the wrong default.

`DistanceMetric::similarity` converts a stored distance into the similarity `jarvis_core::retrieval` scores,
because `jarvis_core::retrieval`'s cosine signal is in `[-1, 1]` and `ADR-0048` recorded that scale. Two of the
three conversions are negations and one is `1 - d`; getting it wrong produces a ranking that is *reversed* or
*shifted* rather than an error, so the conversion lives in one place with both endpoints asserted
(`similarity(0.0) == 1.0`, `similarity(2.0) == -1.0`).

Euclidean has no similarity in the same units, and every reciprocal-shaped alternative invents a constant, so
it negates the distance: that orders identically to `1 / (1 + d)` while staying linear, and it is documented
as an **ordering** rather than a similarity so nothing treats it as one.

### 7. `ascending_is_closer` is a named predicate, not a tautology

It returns `true` for all three metrics, and that is the point: the README says `<#>` is negated *because*
Postgres only supports `ASC` index scans. A caller converting a stored value into a similarity has one place
to ask, so the negation is asserted rather than remembered. A predicate that is currently constant is still
where the knowledge lives.

## Consequences

- **This slice does not create the `memory_embeddings` table, and does not claim pgvector parity.** What it
  delivers is the part that was missing and blocking everything else: the codec, the metric pairing, and the
  refusal for an unstorable dimension. The table, the repository functions, the migration, and the server-mode
  composition are **not written**, so "PostgreSQL plus pgvector backend parity" is **not** achieved and
  `P4-009` is not complete.
- **No SQLx Postgres dependency was added.** Nothing in the workspace opens a Postgres connection, so adding
  the feature would enable code no test exercises and no binary calls — a dependency present without a
  measured requirement, which `AGENTS.md` forbids. The `sqlx` workspace dependency carries only
  `macros`, `migrate`, `runtime-tokio`, and `sqlite-bundled`.
- ~~**No test verifies against a live server.**~~ **CLOSED by `ADR-0053`.** The reason given here was that
  Docker was installed but its daemon was not running. That was true, and the daemon was what blocked the
  slice: starting it made `pgvector/pgvector:pg17` available (PostgreSQL 17.11, vector 0.8.6 — the version
  this record selected), and `crates/jarvis-storage/tests/postgres_embeddings.rs` now runs 14 tests against
  it. **A slice can be blocked by a daemon rather than by a decision**, and this limit was recorded as
  environmental rather than as unavailable work — which is why it was the first thing to retry.
- ~~**The `pgvector` extension is not installed or detected by any migration**, because no Postgres migration
  exists.~~ **CLOSED by `ADR-0053`**, with a measured refinement: the migration runs
  `CREATE EXTENSION IF NOT EXISTS vector`, and the extension is **not trusted**
  (`pg_available_extension_versions.trusted = false`), so a role without superuser fails there — and the
  downstream symptom is `type "vector" does not exist`, which does not name the extension. `CREATE EXTENSION`
  **inside a transaction** was verified to work, which is what lets `sqlx::migrate!` carry it.
- **Recorded limits**, with what `ADR-0053` closed: ~~No `memory_embeddings` table. No repository function for
  a vector read or write. No `CREATE EXTENSION` or `CREATE INDEX` DDL.~~ **all three now exist and are
  measured against a live server.** Still open: **no `hnsw.ef_search` tuning**, so a filtered query returns
  fewer rows than requested — now *measured* rather than quoted, since a 10%-selective filter over 400 rows
  returned at most 40. No iterative index scans. No workspace partitioning, so the README's cross-tenant
  recall note applies to a multi-workspace deployment — and the index is created per width, not per workspace.
