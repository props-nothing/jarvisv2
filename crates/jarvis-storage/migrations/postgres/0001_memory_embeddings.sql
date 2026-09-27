-- JARVIS server-mode schema: memory embeddings, indexed with pgvector.
--
-- `ADR-0003` makes PostgreSQL plus pgvector the server and multi-device backend, and `P4-009` requires
-- parity with the memory behaviour the SQLite backend already has. This migration adds the one table
-- `docs/data/schema.md` names for that purpose (`memory_embeddings`) and nothing else: the memory, entity,
-- and source tables are the SQLite schema's business, and porting them is a separate slice with its own
-- migration rather than an implied part of "vector parity".
--
-- Every claim below was verified against a live server rather than read and assumed:
-- `pgvector/pgvector:pg17`, PostgreSQL 17.11, vector 0.8.6. The probes are recorded in
-- `docs/research/integrations/postgres-pgvector.md`.

-- The extension has to exist before a `vector` column means anything, and its absence is not a
-- self-explanatory failure: without it, `CREATE TABLE ... embedding vector` reports only
-- `type "vector" does not exist`, which does not say an extension is missing. Verified on a live server,
-- along with two facts that make this statement safe here: `CREATE EXTENSION` **works inside a
-- transaction** (`sqlx::migrate!` wraps every migration in one), and `IF NOT EXISTS` makes a repeat a
-- NOTICE rather than an error.
--
-- Recorded limit: the extension is **not trusted** (`pg_available_extension_versions.trusted = false`), so
-- this statement needs a superuser or a preinstalled extension. A deployment whose role cannot install it
-- will fail here, and that failure is the correct one to see — but a better error message would need a
-- pre-flight check this slice does not add. See `docs/research/integrations/postgres-pgvector.md`.
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE memory_embeddings (
    -- The memory this vector belongs to. Deliberately **not** a foreign key to a `memories` table, because
    -- this migration does not create one: the server-mode memory schema does not exist yet, and a
    -- `REFERENCES` clause naming a table nobody creates would make this migration fail for a reason that
    -- has nothing to do with embeddings. The column is the join key for the slice that adds the parent.
    memory_id     TEXT        NOT NULL,
    -- The scope a nearest-neighbour query must filter on. `docs/architecture/memory-and-context.md` makes a
    -- workspace the boundary every read respects, so a vector search without it would be the one query in
    -- the product that can cross it.
    workspace_id  TEXT        NOT NULL,

    -- The metadata that decides comparability, as `docs/architecture/storage.md` requires: "Store embedding
    -- provider/model, dimensions, normalization, input hash, chunker version, and created time. **Never
    -- compare vectors with incompatible metadata.**" These are columns rather than a JSON blob so a query
    -- can *require* agreement instead of trusting that every stored vector came from one model.
    provider      TEXT        NOT NULL,
    model         TEXT        NOT NULL,
    model_version TEXT        NOT NULL,
    dimensions    INTEGER     NOT NULL,
    normalization TEXT        NOT NULL,
    -- The hash of the text this vector was computed from, so a re-embedding of unchanged text is
    -- detectable. Lowercase hex SHA-256, bounded by the CHECK below because it is compared for equality.
    input_hash    TEXT        NOT NULL,

    -- The vector itself, declared at **no fixed width**. `vector(n)` would make the column accept exactly
    -- one dimension, and the metadata above exists precisely because more than one dimension is
    -- legitimate (a provider upgrade changes it). A fixed width would turn "store this new model's
    -- vectors" into a migration. The indexed width is enforced by the partial index below instead.
    embedding     vector      NOT NULL,

    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- One vector per (memory, model) rather than per memory: a re-embedding by a new model adds a row
    -- rather than replacing one, so a comparison across models is not silently available, and the old
    -- vector stays until the new one is proven. The triple is the comparability key, so it is also the
    -- uniqueness key — one row per comparable metadata set.
    PRIMARY KEY (memory_id, provider, model, model_version),

    -- **The invariant that makes the metadata trustworthy.** Without this, a row can declare
    -- `dimensions = 1536` while holding a 3-component vector — verified: such an insert succeeded cleanly
    -- on a live server. A reader comparing `vector_dims(embedding)` against `dimensions` to decide whether
    -- a vector is comparable would then be reading a column that can be wrong, and the wrongness would
    -- look like a legitimate "different model" rather than as corruption.
    --
    -- `vector_dims` is `IMMUTABLE` (verified in `pg_proc`), which is what makes it legal in a CHECK and in
    -- an index predicate. The `WHERE vector_dims(embedding) = n` clause on the index below depends on the
    -- same property.
    CONSTRAINT memory_embeddings_dimensions_match CHECK (vector_dims(embedding) = dimensions),

    -- Bounds `dimensions` to what this product can actually **index**, which is the narrower of the two
    -- limits that exist here.
    --
    -- The `vector` *type* holds up to 16,000 dimensions (`CREATE TABLE ... vector(16384)` is refused with
    -- `dimensions for type vector cannot exceed 16000`, and inserting a 16,001-value vector is refused too —
    -- both measured). pgvector's HNSW index caps at **2,000**, and `jarvis_storage::pgvector::encode`
    -- refuses anything above the indexed cap, so **no code path can write a 2,001-dimension vector**.
    --
    -- A CHECK allowing 2,001 to 16,000 would therefore be a claim about capability that nothing can
    -- exercise: it would read as "JARVIS stores embeddings this large" while the write path refuses them,
    -- and a direct insert that used the room would create a vector the partial index cannot cover — a
    -- retrieval that silently becomes an exact scan. So the bound states the product's real capability, and
    -- supporting a wider model (a `halfvec` column, or binary quantization with re-ranking) is a migration
    -- rather than a value that quietly fits.
    CONSTRAINT memory_embeddings_dimensions_range CHECK (dimensions BETWEEN 1 AND 2000),

    -- Mirrors `jarvis_models::Normalization`'s three wire codes. A CHECK rather than an enum type: the
    -- vocabulary belongs to the domain crate, and a database type would be a second place it is declared.
    -- `unknown` is permitted and is the correct value for a provider that states nothing.
    CONSTRAINT memory_embeddings_normalization CHECK (
        normalization IN ('normalized', 'unnormalized', 'unknown')
    ),

    -- The hash is compared for equality and for nothing else, so its shape is worth constraining: a value
    -- that is not 64 hex characters cannot be a SHA-256 digest, and a shorter string would compare
    -- unequal to a real digest and read as "the text changed".
    CONSTRAINT memory_embeddings_input_hash CHECK (input_hash ~ '^[0-9a-f]{64}$')
);

-- The nearest-neighbour index.
--
-- # Why this is a PARTIAL index over an unconstrained column, and why the alternative is not merely worse
--
-- The obvious index for an unconstrained `vector` column is a cast expression:
--
--     CREATE INDEX ... USING hnsw ((embedding::vector(1536)) vector_cosine_ops);   -- DON'T
--
-- It creates successfully and then **refuses every insert of any other dimension** — measured on a live
-- server, where a table of 1536-dimension rows became completely uninsertable with
-- `ERROR: expected 1536 dimensions, not 3`. The cast is applied on write, before the index decides whether
-- the row is its business.
--
-- Adding `WHERE vector_dims(embedding) = 1536` fixes exactly that: a partial index only covers rows that
-- satisfy its predicate, so a row of another dimension is not a row this index has an opinion about.
-- Measured afterwards: rows of 3, 4, 1536, 2001 and 16000 dimensions all stored in the same column, and
-- the query below still produced `Index Scan using ...` — the same plan a fixed `vector(1536)` column gets.
--
-- So the predicate is load-bearing in both directions, and the query this table is for must **state it
-- too**, since a partial index is only usable when the planner can prove the query's rows satisfy it.
--
-- # Why 1,536 and not 2,000
--
-- 2,000 is pgvector's *index* cap, not a useful default: 2,000-dimension models are rare, and an index at
-- the cap would cover exactly the vectors this project does not have. `text-embedding-3-small` — the model
-- `docs/research/integrations/embeddings.md` researched — returns 1,536, and `embedding.rs`'s
-- `DEFAULT_DIMENSIONS` is that value. `index_ddl(dimensions)` in `embedding_repository.rs` emits this same
-- statement for another width, so supporting a second model is a function call rather than hand-written SQL.
--
-- # Why cosine
--
-- The researched provider normalizes its vectors and recommends cosine, and cosine gives comparable
-- rankings whether or not a future provider normalizes — which a dot product does not. `Normalization`
-- defaults to `Unknown` precisely because assuming normalization is the wrong default, so the one metric
-- that does not need that assumption is the one worth indexing.
--
-- # Recorded limits, both from the pgvector README and both observable in `EXPLAIN`
--
-- An approximate index **trades recall for speed**: "you will see different results for queries after
-- adding an approximate index", so a test may assert neighbours and index usage but not exact recall.
-- And "filtering is applied *after* the index is scanned", with `hnsw.ef_search` defaulting to 40 — so a
-- selective `workspace_id` filter legitimately returns **fewer rows than `LIMIT`**. The verified plan for
-- the metadata-guarded query shows both: `Index Scan using ...` with the predicates in a `Filter:` line,
-- which is the documented post-scan filtering rather than the index's own search. This migration sets no
-- `hnsw.ef_search`, so the default applies.
CREATE INDEX memory_embeddings_embedding_hnsw
    ON memory_embeddings
    USING hnsw ((embedding::vector(1536)) vector_cosine_ops)
    WHERE vector_dims(embedding) = 1536;

-- The workspace-scoped lookup the query's filter needs. The README's own remedy for post-index filtering is
-- "a B-tree index on the filter column (good for low-selectivity conditions)", which is this — without it
-- the filter is applied to whatever the index happened to return, and the recall cliff is wider.
CREATE INDEX memory_embeddings_workspace_idx
    ON memory_embeddings (workspace_id, provider, model, model_version);
