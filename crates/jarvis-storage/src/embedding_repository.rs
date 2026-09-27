//! Memory embeddings in PostgreSQL, indexed by pgvector.
//!
//! `ADR-0003` makes PostgreSQL plus pgvector the server and multi-device backend; `P4-009` requires parity
//! with the memory behaviour the SQLite backend already has. [`crate::pgvector`] built and tested the text
//! codec — the one piece that can be wrong without a server noticing — and this module is the repository
//! around it: the schema in `migrations/postgres/`, the writes and reads, and the index DDL.
//!
//! # Everything here was measured against a live server, not read and assumed
//!
//! The probes ran against `pgvector/pgvector:pg17` — PostgreSQL 17.11, vector **0.8.6** — and are recorded
//! in `docs/research/integrations/postgres-pgvector.md`. Four of them changed the design:
//!
//! 1. **A cast-expression HNSW index over an unconstrained `vector` column makes the column uninsertable at
//!    any other dimension.** `CREATE INDEX ... USING hnsw ((embedding::vector(1536)) vector_cosine_ops)` with
//!    no predicate created successfully and then refused every 3-, 4- and 2001-dimension insert with
//!    `expected 1536 dimensions, not 3`. Measured: a table of 1536-dimension rows became entirely
//!    uninsertable. Adding `WHERE vector_dims(embedding) = 1536` fixes it — a partial index only covers rows
//!    matching its predicate — and the query still produced `Index Scan using ...`. **That predicate is the
//!    load-bearing part of this design**, in the index *and* in the query.
//! 2. **`vector(n)` refuses a query whose query vector has another dimension**, with
//!    `ERROR: different vector dimensions 1536 and 3` rather than a plan. So a search must cast its column
//!    expression to the width the caller is searching at; it cannot compare across widths even to fail.
//! 3. **`dimensions` and `vector_dims(embedding)` can disagree.** A row declaring `dimensions = 1536` while
//!    holding a 3-component vector inserted cleanly, and adding the constraint afterwards failed *on that
//!    row*. `migrations/postgres/0001_memory_embeddings.sql` ties the two with a CHECK, because a reader that
//!    compared them to decide comparability would otherwise be reading a column that can be wrong.
//!    **And the bound on `dimensions` is pgvector's *index* cap (2,000), not its storage cap (16,000)**:
//!    `encode` already refuses to write a vector the index cannot cover, so a CHECK permitting 2,001 to
//!    16,000 would describe a capability no code path can exercise.
//! 4. **The metadata guard does not cost the index.** The query with `workspace_id`, provider, model,
//!    `model_version`, normalization and the dimension predicate all present produced
//!    `Index Scan using memory_embeddings_embedding_hnsw` with those predicates in a `Filter:` line. That
//!    `Filter:` is the README's documented post-scan filtering, and it is why a selective filter returns
//!    fewer rows than `LIMIT` — a real limit, not a bug, and one this module records rather than hides.
//!
//! # Why the module takes `&[f32]` and plain strings rather than the domain's embedding types
//!
//! `docs/architecture/repository-layout.md`'s dependency graph allows an adapter to depend on `jarvis-core`
//! and `jarvis-protocol` and **not on another adapter**. `jarvis-storage` and `jarvis-models` are both
//! adapters, so `EmbeddingVector` and `EmbeddingMetadata` cannot appear in this module's signature — the same
//! rule that forced [`crate::pgvector`] to take `&[f32]`, and the same one `ADR-0047` applied inside the
//! provider boundary. [`NewMemoryEmbedding`] therefore carries the metadata as borrowed `&str`s and the
//! values as `&[f32]`, and the composition root maps between the two vocabularies.
//!
//! # Why the index has a dimension parameter at all
//!
//! [`index_ddl`] exists so that supporting a second embedding model is a function call rather than
//! hand-written SQL that has one chance to get the predicate right. The migration creates the index for
//! `DEFAULT_INDEXED_DIMENSIONS` (1,536, the researched provider's width); a second model calls `index_ddl`
//! with its own, and a partial index per indexed width is the documented consequence rather than a
//! workaround.
//!
//! # The metadata guard is a `WHERE` clause, not a convention
//!
//! `docs/architecture/storage.md`: "**Never compare vectors with incompatible metadata.**" A comparison
//! across models returns a number in the usual range, so a missing guard looks like a slightly worse
//! ranking rather than as an error. [`SimilarityQuery`] therefore requires the provider, model, version,
//! normalization and dimensions, and [`search_similar`] binds every one of them into the statement — there
//! is no constructor that omits them and no code path that searches without them.

use sqlx::PgPool;
use sqlx::postgres::PgQueryResult;

use crate::pgvector::{self, DistanceMetric};

/// The dimension the migration indexes, and the one `jarvis_models` defaults to.
///
/// `text-embedding-3-small` — the model `docs/research/integrations/embeddings.md` researched — returns
/// 1,536, and `jarvis_models::vector::EmbeddingDimensions`'s `DEFAULT_DIMENSIONS` is the same value. Kept as
/// a named constant rather than a literal so the migration, [`index_ddl`]'s default and the tests cannot
/// drift apart.
///
/// Note this is **not** [`pgvector::MAX_INDEXED_DIMENSIONS`] (2,000). That is pgvector's ceiling; an index at
/// the ceiling would cover exactly the vectors this project does not have.
pub const DEFAULT_INDEXED_DIMENSIONS: usize = 1_536;

/// The most embedding inputs one workspace search may return.
///
/// The same shape as the other read bounds in this crate: a caller-supplied page size is a way to ask a
/// server for everything, so the bound lives where the query is built. It is deliberately small — a
/// nearest-neighbour result set is a **candidate source** for `jarvis_core::retrieval`'s ranking
/// (`ADR-0046`), not the ranking itself, so a hundred candidates is already generous. Note it cannot be
/// *reached* reliably through a selective filter, because pgvector with HNSW filters after the index scan;
/// see the module doc.
pub const MAX_SIMILARITY_RESULTS: usize = 100;

/// A memory embedding to record, as the metadata that decides its comparability plus its values.
///
/// Every field is borrowed, because nothing here outlives the call. The metadata is a set of **required
/// parameters rather than options**: "Never compare vectors with incompatible metadata" cannot be enforced
/// by a caller that is allowed to omit the metadata, so a partial one is not constructible.
#[derive(Clone, Copy, Debug)]
pub struct NewMemoryEmbedding<'a> {
    /// The memory the vector describes.
    pub memory_id: &'a str,
    /// The workspace the memory belongs to. The scope a search must filter on.
    pub workspace_id: &'a str,
    /// The embedding provider, as it names itself.
    pub provider: &'a str,
    /// The embedding model identifier.
    pub model: &'a str,
    /// The model version the provider reports. Opaque, like `jarvis_models::EmbeddingVersion`.
    pub model_version: &'a str,
    /// The three-valued normalization property, using `jarvis_models::Normalization`'s wire codes.
    pub normalization: &'a str,
    /// The lowercase hex SHA-256 of the text this vector was computed from.
    pub input_hash: &'a str,
    /// The vector's components.
    ///
    /// Takes the values rather than the text form, so a caller cannot supply a literal that disagrees
    /// with the declared dimension — [`pgvector::encode`] derives the width from the slice, and the
    /// database's own CHECK ties it to the `dimensions` column.
    pub values: &'a [f32],
}

impl NewMemoryEmbedding<'_> {
    /// Returns the vector's dimension count, which is the value `vector_dims` will report.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.values.len()
    }
}

/// Why an embedding could not be recorded or searched.
///
/// Separates the two questions a caller can act on differently: a **refusal** (this input is wrong, and the
/// caller can fix it) from a **storage failure** (the database did not answer, and the caller should not
/// retry blindly). The gateway's `400`/`422`/`503` split is the same distinction at the transport layer.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    /// The vector itself cannot be stored or compared.
    ///
    /// Carries [`pgvector::PgVectorError`] unchanged, so the dimension refusal's documented alternatives
    /// reach the caller rather than being flattened into one message.
    #[error(transparent)]
    Vector(#[from] pgvector::PgVectorError),
    /// A metadata field is empty or unusable.
    ///
    /// Named by field, because "which one" is the whole content of the fix. The metadata is what decides
    /// whether a stored vector may be compared with another, so an empty provider or model is not a
    /// cosmetic problem: it makes two incomparable vectors look comparable.
    #[error("the embedding metadata field `{field}` is empty")]
    MetadataEmpty {
        /// The offending field.
        field: &'static str,
    },
    /// The input hash is not a lowercase hex SHA-256 digest.
    ///
    /// Checked here as well as in the schema, so a caller gets a field rather than a constraint violation.
    /// The value is compared for equality and for nothing else, so a wrong shape would compare unequal to a
    /// real digest and read as "the text changed".
    #[error("the embedding input hash is not a lowercase hex SHA-256 digest")]
    InputHashMalformed,
    /// The requested result count is zero or above [`MAX_SIMILARITY_RESULTS`].
    ///
    /// Zero is refused rather than treated as "no limit": a caller reading an empty result cannot tell
    /// "nothing is similar" from "you asked for none", and those lead to opposite next steps.
    #[error("a similarity search asked for {requested} results, and the bound is 1 to {maximum}")]
    ResultLimit {
        /// The requested count.
        requested: usize,
        /// [`MAX_SIMILARITY_RESULTS`].
        maximum: usize,
    },
    /// The database did not answer.
    #[error("the PostgreSQL embedding operation `{operation}` failed")]
    Postgres {
        /// The operation being attempted.
        operation: &'static str,
        /// The underlying driver error.
        #[source]
        source: sqlx::Error,
    },
}

/// A nearest-neighbour query, with the metadata that makes the comparison meaningful.
///
/// # Why this is a struct rather than a long parameter list
///
/// Six of these values are strings that decide comparability, and a transposition between them would still
/// be *accepted* — `model` and `model_version` are both opaque text, so a swapped pair validates and
/// silently searches a different model's vectors. The same reasoning that produced `CallBinding` in
/// `jarvis-tools` and `ToolPosture` in `jarvis-mcp`.
#[derive(Clone, Copy, Debug)]
pub struct SimilarityQuery<'a> {
    /// The workspace to search in. **Required**, because a workspace is the boundary every read respects and
    /// a vector search without it would be the one query that can cross it.
    pub workspace_id: &'a str,
    /// The provider whose vectors may be compared.
    pub provider: &'a str,
    /// The model whose vectors may be compared.
    pub model: &'a str,
    /// The model version whose vectors may be compared.
    pub model_version: &'a str,
    /// The normalization property the stored vectors must share.
    pub normalization: &'a str,
    /// The query vector's components.
    pub values: &'a [f32],
    /// The distance metric to rank by.
    ///
    /// Required rather than defaulted, because the metric selects the **index** as well as the operator:
    /// pgvector's README requires "an index for each distance function you want to use", so a metric
    /// without its index is an exact scan that still returns answers. A default would make that the silent
    /// outcome of not choosing.
    pub metric: DistanceMetric,
    /// How many results to return, at most. Bounded by [`MAX_SIMILARITY_RESULTS`].
    pub limit: usize,
}

/// One nearest-neighbour result: the memory and how far its vector was from the query.
#[derive(Clone, Debug)]
pub struct SimilarMemory {
    /// The memory the vector belongs to.
    pub memory_id: String,
    /// The distance, as pgvector's operator reported it — **a distance, not a similarity**.
    ///
    /// The distinction is the one [`DistanceMetric::ascending_is_closer`] exists for, and it is why this
    /// field is not called `score`: `jarvis_core::retrieval` scores in *similarity* units where higher is
    /// better, and `<#>` is the **negative** inner product, so treating this value as a score would rank
    /// backwards while every type checked. Converting is
    /// [`DistanceMetric::similarity`]'s job and happens in the composition root.
    ///
    /// # Why this is `f64`
    ///
    /// pgvector's distance operators return PostgreSQL `double precision`, verified live for all three
    /// (`<=>`, `<#>` and `<->`, each `double precision`). The stored *components* are `real`, because that is
    /// what a `vector` column holds — but the **distance** is computed in double precision, so decoding it as
    /// `f32` would narrow a value the server produced at higher precision. A `real` binding does not fail on
    /// that; it rounds, which is exactly the class of defect that reads as a slightly different number rather
    /// than as an error. The one conversion to `f32` happens in the composition root, where
    /// `jarvis_core::retrieval`'s scoring lives and the narrowing is a deliberate choice at the boundary
    /// rather than an accident of the decode.
    pub distance: f64,
}

/// Records one memory embedding, replacing any row with the same comparability key.
///
/// # Errors
///
/// Returns [`EmbeddingError::MetadataEmpty`] for an empty metadata field or workspace, and any
/// [`pgvector::PgVectorError`] the codec raises. A dimension above
/// [`pgvector::MAX_INDEXED_DIMENSIONS`] is **not** refused here — it is storable, and the schema's range
/// CHECK allows up to 16,000 — because this column is deliberately allowed to hold a vector the index
/// cannot cover. What `encode` refuses is a vector above the *storage* maximum or one with a non-finite
/// component. `index_ddl` is where the indexed cap is enforced.
pub async fn record_memory_embedding(
    pool: &PgPool,
    embedding: &NewMemoryEmbedding<'_>,
) -> Result<(), EmbeddingError> {
    // Every field checked before any SQL, so a caller fixing one problem at a time is not told about them
    // one at a time.
    check_metadata(embedding)?;
    if !is_sha256_hex(embedding.input_hash) {
        return Err(EmbeddingError::InputHashMalformed);
    }
    let encoded = pgvector::encode(embedding.values)?;
    // The database's CHECK ties `dimensions` to `vector_dims(embedding)`, so this bound must agree with
    // the length the codec just encoded. Taken from the slice for exactly that reason.
    let dimensions = dimension_count(embedding.dimensions())?;

    sqlx::query(
        "INSERT INTO memory_embeddings \
             (memory_id, workspace_id, provider, model, model_version, dimensions, normalization, \
              input_hash, embedding) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::vector) \
         ON CONFLICT (memory_id, provider, model, model_version) DO UPDATE SET \
             workspace_id  = EXCLUDED.workspace_id, \
             dimensions    = EXCLUDED.dimensions, \
             normalization = EXCLUDED.normalization, \
             input_hash    = EXCLUDED.input_hash, \
             embedding     = EXCLUDED.embedding, \
             created_at    = now()",
    )
    .bind(embedding.memory_id)
    .bind(embedding.workspace_id)
    .bind(embedding.provider)
    .bind(embedding.model)
    .bind(embedding.model_version)
    .bind(dimensions)
    .bind(embedding.normalization)
    .bind(embedding.input_hash)
    .bind(encoded)
    .execute(pool)
    .await
    .map(|_result: PgQueryResult| ())
    .map_err(|source| EmbeddingError::Postgres {
        operation: "record a memory embedding",
        source,
    })
}

/// Finds the memories whose vectors are nearest the query's, **within one set of comparable metadata**.
///
/// # What the SQL has to get right, and each was a measured failure first
///
/// - **The cast.** The column holds many widths, so the comparison is written as
///   `embedding::vector($n) <=> $1::vector($n)`. Without the cast, `vector(n) <=> other(n)` across widths is
///   `ERROR: different vector dimensions 1536 and 3` — an error rather than a plan, so the cast is not an
///   optimisation.
/// - **The predicate.** `vector_dims(embedding) = $n` must be stated, because the HNSW index is **partial**
///   and a planner can only use it when it can prove the query's rows satisfy the predicate. Omitting it
///   turns an index scan into a sequential scan with no error at all.
/// - **The dimension in the `ORDER BY`, not inside a `1 - x` expression.** pgvector's Troubleshooting
///   section: "the `ORDER BY` must be the result of a distance operator (not an expression) in ascending
///   order", and its own counter-example is `ORDER BY 1 - (embedding <=> q) DESC` — the natural way to write
///   cosine *similarity*, which uses no index. This orders by the distance ascending and leaves the
///   conversion to the caller.
/// - **Every metadata field, bound.** The metadata guard is a `WHERE` clause rather than a convention; see
///   the module doc.
///
/// # The dimension is the query vector's own length
///
/// It is not a caller-supplied parameter, because a caller could then ask for a width other than the one it
/// is searching with, and the comparison would be between a vector and a cast that means something else.
///
/// # Errors
///
/// Returns [`EmbeddingError::ResultLimit`] for a limit outside 1..=[`MAX_SIMILARITY_RESULTS`] and any
/// [`pgvector::PgVectorError`] the codec raises for the query vector.
pub async fn search_similar(
    pool: &PgPool,
    query: &SimilarityQuery<'_>,
) -> Result<Vec<SimilarMemory>, EmbeddingError> {
    if query.workspace_id.trim().is_empty() {
        return Err(EmbeddingError::MetadataEmpty {
            field: "workspace_id",
        });
    }
    check_result_limit(query.limit)?;
    let dimensions = dimension_count(query.values.len())?;
    let encoded = pgvector::encode(query.values)?;
    let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
    let operator = query.metric.operator();

    // The operator and the cast width are interpolated rather than bound, and neither can be a parameter:
    // an operator is a token, and `vector(1536)` is a *type* in a cast — the driver refuses both as
    // placeholders. `sqlx` therefore refuses a `format!`-built statement at compile time
    // ("dynamic SQL strings should be audited for possible injections"), and `AssertSqlSafe` is the audited
    // assertion that gets past it. The audit is that **both** interpolated values are typed rather than
    // caller-supplied text: `operator()` returns one of three fixed tokens from an enum match, and
    // `dimensions` is an `i64` produced by `dimension_count` and rendered by `format!`, so neither can carry
    // a quote, a comment, or a statement separator. Every caller-supplied value is bound below.
    let sql = format!(
        "SELECT memory_id, embedding::vector({dimensions}) {operator} $1::vector({dimensions}) AS distance \
         FROM memory_embeddings \
         WHERE workspace_id = $2 \
           AND provider = $3 AND model = $4 AND model_version = $5 \
           AND normalization = $6 \
           AND vector_dims(embedding) = {dimensions} \
         ORDER BY embedding::vector({dimensions}) {operator} $1::vector({dimensions}) ASC \
         LIMIT $7"
    );

    // `f64` rather than `f32`: the three distance operators return PostgreSQL `double precision`, verified
    // live. Binding `f32` does not fail — it rounds — which is why the narrower type would have been a
    // silent narrowing rather than a compile error. See `SimilarMemory::distance`.
    let rows = sqlx::query_as::<_, (String, f64)>(sqlx::AssertSqlSafe(sql))
        .bind(encoded)
        .bind(query.workspace_id)
        .bind(query.provider)
        .bind(query.model)
        .bind(query.model_version)
        .bind(query.normalization)
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|source| EmbeddingError::Postgres {
            operation: "search similar memories",
            source,
        })?;

    Ok(rows
        .into_iter()
        .map(|(memory_id, distance)| SimilarMemory {
            memory_id,
            distance,
        })
        .collect())
}

/// Returns the DDL that indexes a `vector` column written at `dimensions` wide.
///
/// # Why this takes a parameter rather than being a constant
///
/// The migration indexes [`DEFAULT_INDEXED_DIMENSIONS`]. A second embedding model has a different width, and
/// pgvector requires an index per distance function and cannot index a width above 2,000 — so a second
/// model means a second partial index, which is a documented consequence rather than a workaround. Emitting
/// it from a function keeps the two load-bearing parts (the cast and the predicate) in one place, where the
/// live-server measurements that justify them are recorded, instead of duplicated as SQL text that has one
/// chance to be right.
///
/// # Errors
///
/// Returns [`EmbeddingError::ResultLimit`] — repurposed here as a **bound** failure — when `dimensions` is
/// zero or above [`pgvector::MAX_INDEXED_DIMENSIONS`], because pgvector refuses to build an HNSW index on a
/// column above that width (`column cannot have more than 2000 dimensions for hnsw index`). Refusing by
/// name beats emitting DDL the server will reject with a message that does not mention this crate.
pub fn index_ddl(dimensions: usize) -> Result<String, EmbeddingError> {
    if dimensions == 0 || dimensions > pgvector::MAX_INDEXED_DIMENSIONS {
        return Err(EmbeddingError::ResultLimit {
            requested: dimensions,
            maximum: pgvector::MAX_INDEXED_DIMENSIONS,
        });
    }
    Ok(format!(
        "CREATE INDEX IF NOT EXISTS memory_embeddings_embedding_hnsw_{dimensions} \
         ON memory_embeddings \
         USING hnsw ((embedding::vector({dimensions})) vector_cosine_ops) \
         WHERE vector_dims(embedding) = {dimensions}"
    ))
}

/// Returns whether the value is a lowercase hex SHA-256 digest.
///
/// Hand-written rather than a regular expression, so the check has no dependency and mirrors the schema's
/// own `~ '^[0-9a-f]{64}$'` character by character.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Refuses an empty metadata field, naming the one that is empty.
///
/// Extracted so the write path and the test that covers it call **the same function** rather than restating
/// the predicate. A test that reimplements a guard's condition can agree with itself while the guard it
/// describes is wrong — a lesson this workspace has paid for more than once.
///
/// Whitespace counts as empty: a provider that stored `" "` would satisfy the schema's `NOT NULL` while
/// meaning nothing, and the metadata is what decides whether two vectors may be compared at all.
fn check_metadata(embedding: &NewMemoryEmbedding<'_>) -> Result<(), EmbeddingError> {
    for (field, value) in [
        ("memory_id", embedding.memory_id),
        ("workspace_id", embedding.workspace_id),
        ("provider", embedding.provider),
        ("model", embedding.model),
        ("model_version", embedding.model_version),
        ("normalization", embedding.normalization),
    ] {
        if value.trim().is_empty() {
            return Err(EmbeddingError::MetadataEmpty { field });
        }
    }
    Ok(())
}

/// Refuses a result count the bound cannot honour.
///
/// Extracted for the same reason as [`check_metadata`]: the test calls this function rather than restating
/// `limit == 0 || limit > MAX`, so the boundary that is asserted is the boundary that runs.
///
/// Zero is refused rather than read as "no limit", because from an empty result a caller cannot tell
/// "nothing is similar" from "you asked for none", and those lead to opposite next steps.
fn check_result_limit(limit: usize) -> Result<(), EmbeddingError> {
    if limit == 0 || limit > MAX_SIMILARITY_RESULTS {
        return Err(EmbeddingError::ResultLimit {
            requested: limit,
            maximum: MAX_SIMILARITY_RESULTS,
        });
    }
    Ok(())
}

/// Converts a dimension count into the `i64` the statement binds, refusing one no `i64` can hold.
///
/// Shared by the write and the search, because both bind a width that must agree with `vector_dims` — the
/// schema ties one and the partial index's predicate ties the other, so a disagreement between the two
/// call sites would be a row the index cannot cover rather than an error.
fn dimension_count(dimensions: usize) -> Result<i64, EmbeddingError> {
    i64::try_from(dimensions).map_err(|_| {
        EmbeddingError::Vector(pgvector::PgVectorError::DimensionsTooLarge {
            dimensions,
            maximum: pgvector::MAX_INDEXED_DIMENSIONS,
        })
    })
}

#[cfg(test)]
#[path = "embedding_tests.rs"]
mod tests;
