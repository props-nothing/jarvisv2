# ADR-0053: pgvector indexes a partial cast expression, and the vector's width is a capability

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-009` requires PostgreSQL plus pgvector parity with the memory behaviour the SQLite backend already has
(`ADR-0003`, `docs/architecture/storage.md`). `ADR-0051` settled the text codec, because SQLx 0.9.0 has no
`vector` type mapping at all, and left the backend open with a recorded gap: no `memory_embeddings` table, no
repository function, no index DDL, no `postgres` SQLx feature, and **no test against a live server**.

That gap closed because the environment changed: Docker Desktop was installed on this machine but its daemon
was not running, and starting it made a real pgvector server available. Everything below was therefore
**executed** against `pgvector/pgvector:pg17` — PostgreSQL 17.11, vector 0.8.6, the exact version
`docs/research/integrations/postgres-pgvector.md` selected. The probes are recorded in that file's
"Live-Server Findings" section.

Four of them decided the design, and two could not have been learned from the extension's README.

1. **A cast-expression HNSW index without a predicate makes the column uninsertable at any other width.**
   `CREATE INDEX ... USING hnsw ((embedding::vector(1536)) vector_cosine_ops)` created successfully and then
   refused every 3-, 4- and 2001-dimension insert with `expected 1536 dimensions, not 3`. Measured: a table
   of 1,536-dimension rows became **entirely uninsertable**.
2. **`vector(n)` errors on a foreign-dimension query** rather than returning nothing:
   `ERROR: different vector dimensions 1536 and 3`. So a search must cast to a fixed width; it cannot compare
   across widths even to fail.
3. **The three distance operators return `double precision`, not `real`.** The stored components are `real`,
   but the distance is computed in double precision, so binding it as `f32` rounds rather than failing.
4. **`dimensions` and `vector_dims(embedding)` can disagree.** A row declaring `dimensions = 1536` while
   holding a 3-component vector inserted cleanly, and adding the constraint afterwards failed *on that row*.

## Decision

### 1. The index is a PARTIAL index over an unconstrained `vector` column

```sql
CREATE INDEX memory_embeddings_embedding_hnsw
    ON memory_embeddings
    USING hnsw ((embedding::vector(1536)) vector_cosine_ops)
    WHERE vector_dims(embedding) = 1536;
```

**The predicate is load-bearing in both directions, and neither half is optional.**

A partial index covers only the rows matching its predicate, which is what lets one column hold several
widths — the measured failure above. And a planner can only use a partial index when it can prove the query's
rows satisfy the predicate, so `search_similar` states `vector_dims(embedding) = $n` as well. The control test
shows what the predicate earns: the same query **without** it does not name the index in its plan at all,
while the predicated form produces `Index Scan`.

The column itself is declared `vector` with **no fixed width**. `vector(1536)` would make the column accept
exactly one dimension, and the metadata that decides comparability exists precisely because more than one
dimension is legitimate — a provider upgrade changes it. A fixed width would turn "store this new model's
vectors" into a migration, and the metadata columns would describe a distinction the schema forbids.

**A second width gets a second index, from a function.** `index_ddl(dimensions)` emits the same statement with
its own width and index name, so supporting another model is a call rather than hand-written SQL that has one
chance to get the predicate right. This is not a workaround: pgvector's README requires an index per distance
function, and it cannot index one width per index for several widths at once.

### 2. The stored dimension is the *index* cap, not the storage cap

pgvector stores a `vector` up to 16,000 dimensions and indexes one up to 2,000. Both were measured, and the
domain's own `EmbeddingDimensions::MAX` is **16,384** — above even the storage cap.

The table bounds `dimensions` to **2,000**, and `pgvector::encode` refuses above it. The alternative — allowing
2,001 to 16,000 because the type permits it — was rejected because it describes a capability no code path can
exercise:

- `encode` is the only way to build the vector's text form, so no repository write can exceed the cap;
- a direct insert that used the room would create a row **the partial index cannot cover**, which is a
  retrieval that silently stops being a search;
- and a CHECK is read as a statement about what the product can store. One permitting 16,000 would read as
  "JARVIS stores embeddings this large" while the write path refuses them.

An earlier version of this slice had exactly that inconsistency: the schema allowed 16,000 while the codec
refused above 2,000, and the live test that tried to store a 2,001-dimension vector is what surfaced it. The
two bounds now agree, and an offline test asserts the migration does not contain the storage-cap clause.

### 3. `dimensions` and the vector are tied by a CHECK

```sql
CONSTRAINT memory_embeddings_dimensions_match CHECK (vector_dims(embedding) = dimensions),
```

The metadata decides comparability, so a reader asking "may I compare these two vectors" consults `provider`,
`model`, `model_version`, `normalization` and `dimensions`. A column that can disagree with the vector it
describes makes that question answerable wrongly, and the wrongness looks like a legitimate "different model"
rather than as corruption. `vector_dims` is `IMMUTABLE`, which is what makes it legal here and in the index
predicate.

The write path derives the bound from the slice rather than taking a caller's number, so the disagreement is
**unrepresentable** through the repository; the CHECK is asserted by writing raw SQL, which is the only way to
observe a rule that no code path can break.

### 4. The distance is `f64`, and the metadata guard is a `WHERE` clause

pgvector's operators return `double precision`, so `SimilarMemory::distance` is `f64`. Binding `f32` would
round rather than fail, and the single narrowing happens at the retrieval boundary in the composition root,
where `jarvis_core::retrieval`'s integer weighting makes it a deliberate choice rather than an accident of a
decode.

`docs/architecture/storage.md` says "**Never compare vectors with incompatible metadata**", and a comparison
across models returns a number in the usual range — so a missing guard looks like a slightly worse ranking
rather than as an error. [`SimilarityQuery`] therefore requires the provider, model, version, normalization
and the query vector, and `search_similar` binds every one of them. There is no constructor that omits them
and no code path that searches without them. The live test asserts the guard with four rows that are identical
as vectors and differ in exactly one metadata field each, plus a **positive control** so "returns nothing"
cannot pass.

The workspace is in that guard for the same reason it is in every other read: a vector search without it would
be the one query in the product that can cross a workspace boundary.

### 5. The search returns identifiers and distances, not a ranking

It is a **candidate source** for `jarvis_core::retrieval`, which owns the nine signals and the explanation
(`ADR-0046`). Reusing that ranking keeps one implementation of "why was this selected" instead of two, and it
is why the result bound (`MAX_SIMILARITY_RESULTS = 100`) is generous rather than tuned. The field is called
`distance` and not `score` because the conversion is `DistanceMetric::similarity`'s job: `<#>` is the
**negative** inner product, so a caller treating the stored value as a score ranks backwards while every type
checks.

### 6. The SQLx `postgres` feature, enabled with a measured requirement

`AGENTS.md` requires a measured requirement before adding infrastructure, and `P4-009`'s partial state
recorded that enabling `postgres` "would enable code no test exercises". That was true then and is false now:
this slice has a repository, tests, and a live server. `sqlx-postgres` was **already in the lock file**; the
feature adds `hmac 0.13`, `md-5 0.11`, `stringprep 0.1.5` and `whoami 2.1.3`, and introduces a **second `sha2`
(0.11.0)** alongside the pinned 0.10.9 (`cargo tree --invert sha2@0.11.0` names `sqlx-postgres` as the only
consumer). `cargo deny check` reports advisories, bans, licenses and sources all ok; the duplicate is policy
`warn`, as the pre-existing duplicates are.

`tls-*` stays off, and that is a decision rather than an omission: a server mode that talks TLS over a network
is `docs/product/requirements.md`'s multi-device story, and no research record covers its authentication, so a
plaintext loopback connection is the only one this slice can justify. The implementation was verified over
loopback only.

## Consequences

- **The server's behavior is now measured, not remembered.** Eight findings are recorded in
  `docs/research/integrations/postgres-pgvector.md`, including the one the README does not state: a
  non-partial cast index breaks every other width. That file's status moves from `researched` to
  `implemented`, and its unresolved questions are annotated with what this slice answered and what it did not.
- **The live tests skip without a server and can be made to fail.** `ACCEPTANCE_POSTGRES_URL` supplies the
  connection; `ACCEPTANCE_REQUIRE_POSTGRES=1` turns the absence into a failure naming what to start, the same
  shape as `ACCEPTANCE_REQUIRE_FIXTURE_PEER` and `ACCEPTANCE_REQUIRE_BINARIES`. **Both directions were
  falsified:** with no server and no variable, 14 tests pass by skipping; with no server and the variable set,
  the run fails with an actionable message. A test that needs a database must not break a machine without one,
  but a skip that cannot become a failure is how "parity verified" comes to rest on nothing.
- **Each live test gets its own schema and keeps `public` on the search path.** Cargo runs integration tests in
  parallel, and an extension lives in a schema: measured, a search path holding only the scratch schema made
  the migration fail with `type "vector" does not exist` while the extension was installed.
- **The migration is verified by executing it, not by reading it.** `the_migration_applies_...` applies the
  embedded file and then reads `pg_indexes` for the index definition, asserting `USING hnsw`,
  `vector_cosine_ops` and the `WHERE (vector_dims(embedding) = 1536)` clause — so the claim in this ADR is
  tied to what the server built, not to the text that asked for it.

## Limits

- **The extension is not trusted.** `pg_available_extension_versions.trusted = false`, so `CREATE EXTENSION
  vector` needs a superuser or a preinstalled extension. The migration runs it, which means a deployment whose
  role cannot install the extension fails there — and the downstream symptom is `type "vector" does not
  exist`, which does not say an extension is missing. A pre-flight check with a better message is not built.
- **No server-mode composition.** Nothing in `apps/jarvisd` opens a `PgPool`, so this repository has **no
  production caller** — the same gap `P4-009`'s partial state recorded, now narrower: the code exists and is
  measured, but no daemon path reaches it.
- **No migration of existing SQLite data**, no `memories`/`workspace`/`entity` tables in PostgreSQL, and no
  foreign key from `memory_embeddings.memory_id` — a `REFERENCES` clause naming a table nobody creates would
  make the migration fail for a reason unrelated to embeddings. The server-mode memory schema is its own
  slice.
- **`hnsw.ef_search` is left at its default (40)**, which was measured as a real recall bound: a
  10%-selective filter over 400 rows returned at most 40. Nothing tunes it, and the research record's question
  about the right value is still open.
- **One index per width is hand-managed.** `index_ddl` makes the second one correct-by-construction, but
  nothing calls it automatically when a new embedding model appears, and no code detects a width whose rows
  are unindexed.
- **`Normalization` is not enforced against the vector.** The column stores which property the provider
  claimed, and the search requires it to match, but nothing recomputes whether a vector claiming `normalized`
  really is — that is `jarvis-models`' conversion's business.
- **No `halfvec`, binary quantization, subvector indexing or dimensionality reduction.** All four are
  documented ways past the 2,000-dimension index cap, and the refusal names them; none is implemented, so a
  model returning more than 2,000 dimensions cannot be used at all.
