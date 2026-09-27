---
integration: postgres-pgvector
status: implemented
last_verified: 2026-09-27
owners: [storage]
selected_spec_version: pgvector 0.8.6; PostgreSQL 13+ (pgvector's supported range), verified on 17
selected_sdk: SQLx 0.9.0 (feature `postgres`)
---

# PostgreSQL And pgvector

## Scope

`P4-009` requires "PostgreSQL plus pgvector backend parity for the completed memory behavior", following
`ADR-0003` (SQLite local, PostgreSQL plus pgvector server) and `docs/architecture/storage.md`'s
"Store embedding provider/model, dimensions, normalization, input hash, chunker version, and created time.
Never compare vectors with incompatible metadata."

The operations researched are the ones a vector index needs:

1. Create the extension and a column that holds an embedding.
2. Write a vector and read it back without losing a component's value.
3. Find the nearest neighbours to a query vector, with a filter, in a way the planner can index.
4. Store and compare the metadata that makes two vectors comparable at all.

Explicitly **out of scope** for this record and this slice: a hosted/remote server deployment, TLS and
remote access (`docs/product/requirements.md`'s multi-device mode), connection pooling policy for a network
database, backup and restore of a server deployment, replication, partitioning for multitenancy, and any
migration of existing SQLite data into PostgreSQL. The research below is the index and the type, because
those are what "pgvector parity" means and what no existing record covers.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| pgvector README (normative for types, operators, index DDL) | https://raw.githubusercontent.com/pgvector/pgvector/master/README.md | 2026-09-27 | the contract for the whole record |
| pgvector changelog (version and deprecation evidence) | https://raw.githubusercontent.com/pgvector/pgvector/master/CHANGELOG.md | 2026-09-27 | selected version, release dates, breaking changes |
| pgvector Docker tags (which Postgres majors ship which extension version) | the README's Docker section | 2026-09-27 | compatibility between extension and server |
| SQLx Postgres driver | https://docs.rs/sqlx/0.9.0/sqlx/postgres/index.html | 2026-09-27 | driver surface, `PgPool`, transactions, COPY |
| SQLx Postgres type mappings (normative for which Rust types need no help) | https://docs.rs/sqlx/0.9.0/sqlx/postgres/types/index.html | 2026-09-27 | **the decisive negative finding**: no `vector` mapping |
| SQLite/SQLx record (the local backend this must reach parity with) | [sqlite-sqlx.md](sqlite-sqlx.md) | 2026-09-20 | the behaviour being reproduced |
| Architecture contract | `docs/architecture/storage.md`, `docs/data/schema.md`, `ADR-0003` | 2026-09-27 | JARVIS-side requirements |

`llms.txt` discovery: `postgresql.org/llms.txt` and a `pgvector` project `llms.txt` were **not found**.
PostgreSQL's own manual is at `postgresql.org/docs/current/`; the extension's authoritative contract is its
repository README and CHANGELOG, both fetched above. Recorded as `not found` rather than inferred.

## Verified Contract

### Operations And Transport

**The extension.** `CREATE EXTENSION vector;` once per database. The extension is a Postgres extension, not a
service, so it is in-process with the server and uses the server's own transport; there is no endpoint.

**The type.** `vector(n)` is a fixed-dimension column type; `vector` without `(n)` accepts vectors of
differing dimensions in one column. Storage is `4 * dimensions + 8` bytes per vector (single precision), and
**vectors can have up to 16,000 dimensions**, but *indexed* vectors are capped lower — see Limits.

**Index types.** `hnsw` and `ivfflat`. HNSW "has better query performance than IVFFlat (in terms of
speed-recall tradeoff), but has slower build times and uses more memory", and "can be created without any
data in the table since there isn't a training step like IVFFlat". IVFFlat "has faster build times and uses
less memory than HNSW, but has lower query performance", and its recall depends on creating it *after* the
table has data and on choosing `lists` (`rows / 1000` up to 1M rows, `sqrt(rows)` above).

**Operators.** `<->` L2, `<#>` **negative** inner product ("Note: `<#>` returns the negative inner product
since Postgres only supports `ASC` order index scans on operators"), `<=>` cosine distance, `<+>` L1 (0.7.0),
`<~>` Hamming and `<%>` Jaccard (bit vectors, 0.7.0). Cosine *similarity* is `1 - (a <=> b)`.

**Indexed dimensions.** `vector` up to **2,000** dimensions for both HNSW and IVFFlat; `halfvec` up to 4,000;
`bit` up to 64,000; `sparsevec` up to 1,000 non-zero elements. Above 2,000 dimensions the documented options
are `halfvec`, half-precision indexing, binary quantization, indexing subvectors, or dimensionality
reduction.

**Distance-operator classes.** `vector_l2_ops`, `vector_ip_ops`, `vector_cosine_ops`, `vector_l1_ops`;
`halfvec_*` and `sparsevec_*` equivalents; `bit_hamming_ops`, `bit_jaccard_ops`. One index per distance
function, because "Add an index for each distance function you want to use."

**Exact vs approximate.** "By default, pgvector performs exact nearest neighbor search, which provides
perfect recall." An index trades recall for speed, and "you will see different results for queries after
adding an approximate index" — recorded because it changes what a test may assert.

**Why an index is or is not used (Troubleshooting).** "The query needs to have an `ORDER BY` and `LIMIT`, and
the `ORDER BY` must be the result of a distance operator (not an expression) in ascending order." The
documented counter-example is exactly the shape a careless hybrid search would produce:
`ORDER BY 1 - (embedding <=> '[3,1,2]') DESC` uses **no index**. This is the single most exploitable fact in
the record: the natural way to express cosine *similarity* destroys the index, and the fix is to rank by
`<=>` ascending and convert afterwards.

**Filtering (the part that decides the query shape).** "With approximate indexes, filtering is applied
*after* the index is scanned. If a condition matches 10% of rows, with HNSW and the default `hnsw.ef_search`
of 40, only 4 rows will match on average." Documented remedies: a B-tree index on the filter column (good
for low-selectivity conditions), partial indexes (a few distinct values), partitioning (many values), or
**iterative index scans** (`SET hnsw.iterative_scan = strict_order | relaxed_order`, 0.8.0), which
"automatically scan more of the index until enough results are found" bounded by `hnsw.max_scan_tuples`
(20,000 default) and `hnsw.scan_mem_multiplier` (1 default).

**NULL and zero vectors are not indexed** ("NULL vectors are not indexed (as well as zero vectors for cosine
distance)"). A memory with no embedding therefore cannot be found by a vector index, which is why the index
must be a *candidate source* and not the only path.

**Multitenancy.** "sharing an approximate index between tenants means vectors from one tenant can affect
recall (and speed) for other tenants. For tenant isolation, use list partitioning or separate tables." This
is a recall/isolation consequence, not a leak, and it is relevant because JARVIS has workspaces.

### Authentication And Authorization

Postgres authenticates the **connection**, and pgvector adds no authentication of its own: "Use pgvector from
any language with a Postgres client." There is no scope, token, webhook, or callback. The secrets are the
connection string's user and password, which is precisely the value `jarvis_storage::config::redact` already
masks (DSN userinfo password) — the one secret form that existing redactor does cover.

Authorization is Postgres's own: `GRANT`s on the extension, the schema, and the table. **No least-privilege
plan is verified in this record**, because JARVIS's server mode has no researched authentication story yet;
that is an unresolved question rather than an assumption.

### Limits And Failure Semantics

- **Dimensions**: 16,000 stored, but **2,000** for an indexed `vector` column. JARVIS's own
  `EmbeddingDimensions::MAX` is **16,384** — *above* both pgvector's storage limit and far above the indexed
  limit. So a JARVIS-valid embedding can be **unstorable and unindexable**, and the mapping must refuse it by
  name rather than let the server reject it.
- **`hnsw.ef_search` default 40** limits how many results a filtered query can return; fewer results than
  requested is the documented symptom, not an error.
- **`ivfflat.probes` default 1**, and "If this is lower than `ivfflat.probes`, `ivfflat.probes` will be
  used."
- **Index build**: "Indexes build significantly faster when the graph fits into `maintenance_work_mem`", and a
  `NOTICE: hnsw graph no longer fits into maintenance_work_mem after N tuples` is emitted when it does not.
  Parallel builds are controlled by `max_parallel_maintenance_workers` (2 default).
- **Replication**: yes — "pgvector uses the write-ahead log (WAL), which allows for replication and
  point-in-time recovery."
- **Idempotency and errors**: not specified for `COPY` or insert; the extension raises Postgres errors (e.g.
  a dimension mismatch is a Postgres error, not a pgvector-specific code). No provider request id exists.

### Data And Compliance

What leaves JARVIS: **nothing**, in the local/server sense that matters here — pgvector runs inside the
Postgres server JARVIS talks to, so a vector never reaches a third party. `ADR-0003`'s server mode is the
user's own server. Retention and deletion are ordinary SQL, and `DELETE FROM` removes the vector with the row
because the vector is a column. **Provider-side copies are still out of reach** — the *embedding provider* saw
the input text (`docs/research/integrations/embeddings.md`), and no pgvector decision changes that. Pricing
is the Postgres host's, not the extension's; pgvector is open source and free.

### Versions And Deprecations

- **pgvector 0.8.6**, released **2026-07-29** — the selected version. `0.8.7` is *unreleased* at the time of
  reading ("0.8.7 (unreleased) — Fixed error with `avg` aggregate when no matching rows"), so 0.8.6 is the
  newest released version.
- Postgres majors: the Docker tags enumerate `pg13` through `pg18`, so **Postgres 13 through 18** ship a
  prebuilt extension.
- Breaking changes in the recent history that matter: **0.8.0 dropped support for Postgres 12** and added
  iterative index scans; **0.6.0 dropped Postgres 11** and "Changed storage for vector from `extended` to
  `external`"; **0.4.0 changed the text representation for vector elements to match `real`** and raised the
  max dimensions from 1024 to 16,000 and the max indexed dimensions from 1024 to 2000. So the 2,000 indexed
  cap is a 0.4.0-era decision, not a documented invariant of 0.8.
- **0.3.1 required recreating all `ivfflat` indexes** when upgrading to it, which is the precedent for
  "an index upgrade may need a rebuild". HNSW has had several corruption/memory fixes (0.8.3 "Fixed possible
  index corruption with HNSW vacuuming", 0.8.4 "Fixed `hnsw graph not repaired` error with HNSW vacuuming").
  Recorded because it argues for a **rebuildable** index, which `docs/architecture/storage.md` already
  requires ("Full-text and vector data are rebuildable indexes").
- Upgrade path: `ALTER EXTENSION vector UPDATE;`, and the installed version is `SELECT extversion FROM
  pg_extension WHERE extname = 'vector';`.

## JARVIS Mapping

| pgvector concept | JARVIS type | Notes |
| --- | --- | --- |
| `vector(n)` column | `jarvis_models::EmbeddingVector` values | serialized as pgvector's text form; SQLx has **no** mapping |
| the dimension in `vector(n)` | `EmbeddingDimensions` | JARVIS allows 16,384; **indexed** pgvector allows 2,000 — a mapping refusal is required |
| `<=>` cosine distance | `jarvis_core::retrieval`'s cosine signal | JARVIS computes cosine *similarity* in Rust; the stored value is a distance, so the conversion happens in the adapter |
| embedding provider/model/version/normalization/input hash | `jarvis_models::EmbeddingMetadata` | the `storage.md` list, already modelled; the index must carry it so a comparison cannot cross metadata |
| `workspace_id` | `jarvis_core::WorkspaceId` | the filter a nearest-neighbour query must apply, and the multitenancy note above says a shared index affects recall across workspaces |
| the `memory_embeddings` table | `docs/data/schema.md`'s row | memory id, embedding metadata, input hash, vector, created timestamp |

**Decisions taken here (JARVIS-side, not external facts):**

1. **The vector is stored as a **string literal**, not as a binary-encoded value, because SQLx 0.9.0 has no
   `vector` type.** The verified type mapping table for SQLx's Postgres driver lists `bool`, the integers,
   `f32`/`f64`, strings, `BYTEA`, `UUID`, `JSON`/`JSONB`, `TIMESTAMPTZ` and others — and **no vector type of
   any kind**. So a hand-written encoder for pgvector's text form (`[1,2,3]`) is unavoidable, and the
   alternative (binding an untyped parameter the driver cannot check) would lose the round-trip guarantee.
   The encoder is the code that needs the falsifying test.
2. **Cosine is the distance.** `docs/research/integrations/embeddings.md` records that the researched
   provider recommends cosine similarity and normalizes its vectors. Cosine therefore gives comparable
   results whether or not a future provider normalizes, which a dot product does not — and
   `Normalization::Unknown`'s doc already says why assuming normalization is the wrong default.
3. **Refuse above 2,000 dimensions rather than store and fail to index.** A column that accepts a vector the
   index cannot hold would make "we have pgvector" true and retrieval silently exact-scan over every row,
   which is a performance claim with no evidence behind it.
4. **Metadata is compared before vectors are.** `storage.md`'s "Never compare vectors with incompatible
   metadata" is the rule; the index carries the metadata as columns so the join can require it, rather than
   trusting that every stored vector came from one model.
5. **The index is a candidate source, not the answer.** The query returns identifiers and distances; ranking
   stays in `jarvis_core::retrieval`, where the nine signals and the explanation live (`ADR-0046`). Reusing
   the ranking keeps one implementation of "why was this selected" instead of two.

## Rejected Alternatives

- **A pgvector-specific Rust client crate** (for example `pgvector-rust`, listed in the README's languages
  table). Rejected: it adds a dependency for one text encoder, and `AGENTS.md` requires a measured
  requirement before adding infrastructure. The encoder is about twenty lines and is exactly what the
  falsifying test should cover.
- **Storing the vector as `BYTEA`** (SQLx *can* bind `Vec<u8>`). Rejected: the column would no longer be a
  `vector`, so no distance operator and no index would apply, which removes the entire reason for pgvector.
- **`halfvec`** to reach 4,000 indexed dimensions. Rejected for now: it halves precision, and no researched
  provider returns more than 3,072 dimensions (`text-embedding-3-large`), so the 2,000 cap is not yet
  binding for a real model. Recorded as the documented escape hatch if one appears.
- **IVFFlat as the default index.** Rejected: it needs existing data to train, so a fresh deployment would
  have a useless index and a silent recall cliff; HNSW can be created on an empty table.
- **`ORDER BY 1 - (embedding <=> q) DESC`**, the natural way to sort by similarity. Rejected because the
  README's Troubleshooting section names it as the shape that uses no index at all.
- **One index shared across workspaces.** Rejected on the README's own multitenancy note (cross-tenant recall
  effects) — pending the unresolved scaling question below.

## Verification Plan

- **offline schema/fixture tests**: the encoder's round-trip (`encode(decode(x)) == x`) and its refusals
  (empty, non-finite, a dimension above the indexed cap). These need no server.
- **independent conformance test**: a server-backed test that inserts a known vector, reads it back, and
  asserts component equality — the cheapest test that would disprove the encoder's central assumption, since
  a wrong text form either errors or silently reorders components.
- **index-usage test**: `EXPLAIN` a nearest-neighbour query and assert the plan mentions the index, which is
  what falsifies "the query shape uses the index". The README's Troubleshooting section is the discriminator.
- **filtered-recall test**: a filtered query that would return fewer rows than `hnsw.ef_search` allows,
  asserting fewer results rather than an error — the documented behaviour that a naive test would call a bug.
- **metadata guard test**: two rows with different metadata, asserting the query cannot return the
  incomparable one.
- **opt-in live smoke test**: a real server, gated behind an environment variable and skipped otherwise, so
  CI without a database still passes and no test claims a server it did not have.

## Unresolved Questions

1. **Does a JARVIS-valid dimension between 2,001 and 16,384 need storing at all?** Impact: the mapping either
   refuses such an embedding or stores it unindexed. Blocks: nothing today, because no researched model
   exceeds 3,072. It must be answered before a model with more than 2,000 dimensions is enabled, so the code
   refuses by name today instead of guessing.
2. **Which Postgres major is the supported minimum for server mode?** pgvector supports 13+ and the Docker
   tags start at 13, but no JARVIS document names a minimum. Impact: the migration DDL and any `GENERATED`
   or `MERGE` usage. Blocks: nothing in this slice, which uses no version-specific SQL.
3. **Is `hnsw.ef_search` raised per query, and to what?** The README documents `SET LOCAL` for one query, but
   the right value depends on recall requirements no document states. Impact: filtered-query recall. Blocks:
   a production recall claim. The code sets nothing and the limit is recorded.
4. **Is there a researched server-mode authentication and TLS story?** Not in this record, and
   `docs/product/requirements.md`'s multi-device mode needs one. Impact: whether a remote PostgreSQL
   connection may exist at all. Blocks: remote deployment, not this slice, which is deliberately local.
5. **Does pgvector need the `vector` extension installed before migrations run, and how does JARVIS detect its
   absence?** The README says `CREATE EXTENSION` once per database, but says nothing about the failure a
   migration sees when the extension is missing. Impact: the error a user gets. Blocks: a good error message,
   not correctness — the migration would fail, which is visible.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-27 | pgvector 0.8.6 (2026-07-29); SQLx 0.9.0; Postgres 13-18 per Docker tags | First record. pgvector 0.8.7 is unreleased. **SQLx 0.9.0 has no `vector` type mapping** (verified against the driver's type table), so a text encoder is required. Indexed `vector` capped at 2,000 dimensions while JARVIS allows 16,384. `ORDER BY 1 - (v <=> q) DESC` does not use the index. | storage |
| 2026-09-27 | **Live: `pgvector/pgvector:pg17`, PostgreSQL 17.11, vector `0.8.6`** | **Implementation-time probes; several corrected or sharpened the claims above. See "Live-Server Findings" below.** | storage |

## Live-Server Findings

Everything below was **executed** against `pgvector/pgvector:pg17` (PostgreSQL 17.11, vector 0.8.6) while
implementing `P4-009`. The record above is what the documentation says; this section is what the server does.
Four of these changed the implementation, and two could not have been learned from the README.

### 1. A cast-expression index WITHOUT a predicate makes the column uninsertable at any other width

This is the finding the schema depends on, and the README does not mention it.

```sql
CREATE TABLE t (embedding vector);                                    -- no fixed width
CREATE INDEX ON t USING hnsw ((embedding::vector(1536)) vector_cosine_ops);  -- NO predicate
INSERT INTO t VALUES ('[1,2,3]');
--> ERROR:  expected 1536 dimensions, not 3
```

The cast is evaluated on **write**, so the index claims every row regardless of its dimension. Measured
consequence: a table of 1,536-dimension rows became **entirely uninsertable** at any other width. Adding a
predicate fixes it:

```sql
CREATE INDEX ON t USING hnsw ((embedding::vector(1536)) vector_cosine_ops)
  WHERE vector_dims(embedding) = 1536;
```

A partial index covers only rows matching its predicate, so a row of another dimension is not a row this
index has an opinion about. Measured afterwards: **3-, 4-, 1536-, 2001- and 16000-dimension vectors all
inserted into the same column**, and the nearest-neighbour query still produced
`Index Scan using ...` — the same plan a fixed `vector(1536)` column gets. The query must state the predicate
too, because a planner can only use a partial index when it can prove the query's rows satisfy it.

### 2. `vector(n)` ERRORS on a foreign-dimension query rather than returning nothing

`SELECT ... ORDER BY embedding <=> '[0.5,0.5,0.5]'` against a column holding 1,536-dimension vectors:

```
ERROR:  different vector dimensions 1536 and 3
```

So the query must cast to a fixed width, and the cast is not an optimisation: without it a search at a width
nobody stored fails at the server rather than returning no neighbours. (With the cast
`embedding::vector(1536) <=> $1::vector(1536)`, and the `vector_dims` predicate, a differently-sized query
vector is a well-formed query that finds nothing — which is the correct outcome.)

### 3. The three distance operators return `double precision`, not `real`

```
cosine_type      | ip_type          | l2_type
-----------------+------------------+------------------
double precision | double precision | double precision
```

The stored *components* are `real` (that is what a `vector` column holds), but the **distance** is computed
in double precision. Binding it as `f32` does not fail — it rounds — which is the class of defect that reads
as a slightly different number rather than as an error. `SimilarMemory::distance` is therefore `f64`, and the
single narrowing to `f32` happens at the retrieval boundary in the composition root.

### 4. `dimensions` and `vector_dims(embedding)` can disagree, and `vector_dims` is valid in a CHECK

A row declaring `dimensions = 1536` while holding a 3-component vector **inserted cleanly**; adding
`CHECK (vector_dims(embedding) = dimensions)` afterwards failed *on that row*. `vector_dims` is
`IMMUTABLE` (`pg_proc.provolatile = 'i'`), which is why it is legal in a CHECK and in a partial-index
predicate. `migrations/postgres/0001_memory_embeddings.sql` therefore ties the two columns, so a reader that
compares them to decide comparability is not reading a column that can lie.

### 5. Both dimension caps, measured

| Declaration | Result |
| --- | --- |
| `vector(16000)` | accepted |
| `vector(16001)` | `ERROR: dimensions for type vector cannot exceed 16000` |
| `vector(16384)` (the domain's `EmbeddingDimensions::MAX`) | refused by the same error |
| inserting a 16,001-value vector | `ERROR: vector cannot have more than 16000 dimensions` |
| `hnsw` over `vector(2000)` | accepted |
| `hnsw` over `vector(2001)` | `ERROR: column cannot have more than 2000 dimensions for hnsw index` |

So the domain's 16,384 maximum is **above pgvector's storage cap as well as its index cap** — the gap is
wider than the record first stated. The table's `dimensions` CHECK bounds to **2,000** (the index cap) rather
than 16,000 (the storage cap), because `jarvis_storage::pgvector::encode` already refuses to write a vector
the index cannot cover: a bound of 16,000 would describe a capability no code path can exercise.

### 6. The extension is not trusted, and `CREATE EXTENSION` works inside a transaction

`pg_available_extension_versions`: `superuser = true`, **`trusted = false`**. So `CREATE EXTENSION vector`
needs a superuser or a preinstalled extension, and a deployment whose role cannot install it fails there.
Downstream the symptom is `type "vector" does not exist` for every `vector` column, which does **not** say an
extension is missing — answering open question 5 partially: the failure is visible but not self-explanatory.

`BEGIN; CREATE EXTENSION IF NOT EXISTS vector; COMMIT` **succeeded**, so `sqlx::migrate!` (which wraps each
migration in a transaction) can carry the statement. `IF NOT EXISTS` makes a repeat a `NOTICE`.

Also measured, and it shaped the test harness rather than the product: with `search_path` set to a scratch
schema only, the migration failed with `type "vector" does not exist at line 270` **while the extension was
installed in `public`**. An extension lives in a schema, so a scratch schema's search path must retain
`public`.

### 7. The metadata-guarded query uses the index, and the filters appear as `Filter:`

```
Limit
  ->  Index Scan using memory_embeddings_embedding_hnsw on memory_embeddings
        Order By: ((embedding)::vector(1536) <=> $1::vector(1536))
        Filter: ((workspace_id = 'workspace-a'::text) AND (provider = 'openai'::text) AND
                 (model = 'text-embedding-3-small'::text) AND (model_version = '2024-02-01'::text) AND
                 (normalization = 'normalized'::text))
```

All six guards — workspace, provider, model, model_version, normalization and the dimension predicate —
leave the index in use. The `Filter:` line **is** the documented post-scan filtering, and it is why a
selective filter returns fewer rows than `LIMIT`. A B-tree index on
`(workspace_id, provider, model, model_version)` is created alongside, which is the README's own remedy for
low-selectivity filter conditions.

### 8. The control that shows where the index comes from

The same query **without** the `vector_dims` predicate does not name the index in its plan at all, while the
predicated form produces `Index Scan`. That is the measurement behind "the predicate is load-bearing in the
index *and* in the query", and it is asserted by `the_query_uses_the_index_because_of_the_predicate`.

### Effect on the record's unresolved questions

- **Q1 (a JARVIS-valid dimension between 2,001 and 16,384)** — *answered for now*: refused by
  `pgvector::encode`, and the table's CHECK agrees. Storing one would be a row that is never indexed.
- **Q2 (which Postgres major)** — still open; the probes used 17.
- **Q3 (`hnsw.ef_search`) — still open**, and measured as a real effect: a 10%-selective filter on 400 rows
  returned at most 40. Nothing sets it.
- **Q4 (server-mode authentication over a network)** — still open, and the reason the SQLx `tls-*` features
  stay off; the implementation was verified over loopback only.
- **Q5 (how is a missing extension detected)** — *partially answered*: the migration installs it, and the
  extension is not trusted, so a non-superuser deployment fails at `CREATE EXTENSION` rather than later. A
  pre-flight check that produces a better message is still unbuilt.
