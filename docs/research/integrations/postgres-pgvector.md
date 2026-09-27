---
integration: postgres-pgvector
status: researched
last_verified: 2026-09-27
owners: [storage]
selected_spec_version: pgvector 0.8.6; PostgreSQL 13+ (pgvector's supported range)
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
