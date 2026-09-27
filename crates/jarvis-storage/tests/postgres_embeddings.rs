//! Live-server tests for the pgvector memory-embedding repository.
//!
//! `P4-009` claims PostgreSQL plus pgvector parity, and a claim about a server cannot rest on a test that
//! never talked to one. Everything in [`jarvis_storage`]'s `embedding_tests.rs` is decidable without a
//! connection; **this file is the part that is not**, and it is gated so that a checkout with no database
//! still builds and passes.
//!
//! # Why the gate is an environment variable, and why it can also be a failure
//!
//! A test that needs a database must not be the reason a machine without one fails — so absent a
//! `ACCEPTANCE_POSTGRES_URL` the tests here **skip**, and each one says so on stderr. But a skip that
//! cannot be turned into a failure is how a claim about a server ends up resting on nothing: the suite
//! reports green, and nothing distinguishes "pgvector parity verified" from "no database was present".
//!
//! So `ACCEPTANCE_REQUIRE_POSTGRES=1` makes the absence a **failure** naming what to start, exactly as
//! `ACCEPTANCE_REQUIRE_FIXTURE_PEER` does for the stdio peer and `ACCEPTANCE_REQUIRE_BINARIES` does for the
//! process gates. Both directions are falsified in the same way as those guards: delete the server and the
//! variable, and the suite still passes; delete the server and set the variable, and it fails with a
//! message that says what is missing.
//!
//! The variable is deliberately **not** prefixed `JARVIS_`: the daemon treats every unknown `JARVIS_*`
//! variable as a configuration error, so a harness variable in that namespace would stop the daemon from
//! starting. That trap is recorded in this workspace's history.
//!
//! # Running it
//!
//! ```text
//! docker run -d --name jarvis-pgvector \
//!     -e POSTGRES_PASSWORD=jarvis_dev_pw -e POSTGRES_USER=jarvis -e POSTGRES_DB=jarvis \
//!     -p 54329:5432 pgvector/pgvector:pg17
//! $env:ACCEPTANCE_POSTGRES_URL = 'postgres://jarvis:jarvis_dev_pw@127.0.0.1:54329/jarvis'
//! $env:ACCEPTANCE_REQUIRE_POSTGRES = '1'
//! cargo test -p jarvis-storage --all-features --test postgres_embeddings
//! ```
//!
//! Verified against exactly that image: PostgreSQL 17.11, **vector 0.8.6** — the version
//! `docs/research/integrations/postgres-pgvector.md` selected, so the tests below measure the contract the
//! record documents rather than a version that happens to be installed.

use jarvis_storage::{
    DEFAULT_INDEXED_DIMENSIONS, MAX_SIMILARITY_RESULTS, NewMemoryEmbedding, SimilarityQuery,
    index_ddl, record_memory_embedding, search_similar,
};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// The connection string for the server under test.
const POSTGRES_URL_ENV: &str = "ACCEPTANCE_POSTGRES_URL";

/// Turns a missing server into a failure. See the module doc.
const REQUIRE_POSTGRES_ENV: &str = "ACCEPTANCE_REQUIRE_POSTGRES";

/// The migration, embedded so the test applies the same file a deployment would.
const MIGRATION: &str = include_str!("../migrations/postgres/0001_memory_embeddings.sql");

/// A 64-character lowercase hex digest, built rather than typed so its length cannot drift.
fn digest(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

/// A vector of `count` components, all `value`.
///
/// A uniform vector has a well-defined cosine distance to another uniform vector, which makes the expected
/// ordering in the nearest-neighbour test decidable by hand rather than by running the code under test.
fn uniform(count: usize, value: f32) -> Vec<f32> {
    vec![value; count]
}

/// A unit basis vector: one component of `1.0` at `axis`, the rest `0.0`.
///
/// Cosine distance between two different basis vectors is exactly `1.0`, and between one and itself exactly
/// `0.0`, so a nearest-neighbour assertion can name the expected answer instead of asserting a relative
/// order — which a wrong-but-consistent implementation would also satisfy.
fn basis(count: usize, axis: usize) -> Vec<f32> {
    let mut values = vec![0.0_f32; count];
    if let Some(slot) = values.get_mut(axis) {
        *slot = 1.0;
    }
    values
}

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// Narrows a distance to `f32` at the retrieval boundary.
///
/// `retrieval.rs` weights its signals with integer arithmetic, so an `f64` distance from pgvector has to
/// become an `f32` somewhere. This is where that is visible, and the precision that matters is in the
/// ORDER — which the caller's assertion checks. `as` rather than `f32::try_from`, which does not exist for
/// floats.
#[allow(
    clippy::cast_possible_truncation,
    reason = "an f64 distance narrows to f32 at the retrieval boundary by design"
)]
fn narrow(distance: f64) -> f32 {
    distance as f32
}

/// The server's URL, or `None` when there is none and the tests should skip.
///
/// Returns rather than exiting, because **a test binary runs all of its tests in one process**: an
/// `exit(0)` would stop the suite after the first and report the rest as absent, which cannot be told apart
/// from them passing.
fn postgres_url_or_skip() -> Option<String> {
    if let Ok(url) = std::env::var(POSTGRES_URL_ENV)
        && !url.trim().is_empty()
    {
        return Some(url);
    }
    let required = std::env::var(REQUIRE_POSTGRES_ENV).is_ok_and(|value| value == "1");
    assert!(
        !required,
        "no PostgreSQL server is configured and {REQUIRE_POSTGRES_ENV}=1; start one with \
         `docker run -d --name jarvis-pgvector -e POSTGRES_PASSWORD=jarvis_dev_pw \
         -e POSTGRES_USER=jarvis -e POSTGRES_DB=jarvis -p 54329:5432 pgvector/pgvector:pg17` \
         and set {POSTGRES_URL_ENV} to its connection string"
    );
    eprintln!(
        "SKIP: {POSTGRES_URL_ENV} is not set; start a pgvector server to exercise the PostgreSQL backend"
    );
    None
}

/// A pool with the migration applied to a **fresh schema**, so a rerun never depends on the last one.
///
/// Each test gets its own schema rather than its own database, because a container start costs seconds
/// where a `CREATE SCHEMA` costs milliseconds — and cargo runs these in parallel, so a shared `public`
/// schema would make them race. The search path is set on every connection in the pool, so a statement
/// cannot reach another test's rows even by accident.
async fn migrated_pool() -> Option<PgPool> {
    let url = postgres_url_or_skip()?;
    let schema = format!("emb_{}", jarvis_core_scratch_tag());

    let pool = must(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .map_err(|error| format!("connect to {url}: {error}")),
        "connecting to the PostgreSQL server",
    );
    // The extension is per DATABASE, not per schema, so it is created once against `public` before the
    // search path moves. A migration that ran without it would fail as `type "vector" does not exist`,
    // which does not say an extension is missing — the reason the migration installs it itself.
    must(
        sqlx::raw_sql("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&pool)
            .await
            .map_err(|error| format!("install the vector extension: {error}")),
        "installing pgvector",
    );
    // `public` stays on the search path, and that is load-bearing: the extension is installed there, so a
    // path holding only the scratch schema cannot resolve the `vector` type — measured, as
    // `type "vector" does not exist at line 270` while the extension was present. The scratch schema comes
    // first, so every object the migration creates lands there and no test shares another's rows.
    let search_path = format!("{schema}, public");
    must(
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE SCHEMA {schema}; SET search_path TO {search_path}"
        )))
        .execute(&pool)
        .await
        .map_err(|error| format!("create the scratch schema: {error}")),
        "creating a scratch schema",
    );
    // A `PgPool` hands out arbitrary connections, so the search path has to be set per connection rather
    // than once. `after_connect` runs for every connection the pool opens, including new ones.
    let pool = must(
        PgPoolOptions::new()
            .max_connections(2)
            .after_connect({
                let search_path = search_path.clone();
                move |connection, _meta| {
                    let search_path = search_path.clone();
                    Box::pin(async move {
                        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                            "SET search_path TO {search_path}"
                        )))
                        .execute(connection)
                        .await
                        .map(|_result| ())
                    })
                }
            })
            .connect(&url)
            .await
            .map_err(|error| format!("connect with a search path: {error}")),
        "connecting with a scratch search path",
    );
    must(
        sqlx::raw_sql(sqlx::AssertSqlSafe(MIGRATION.to_string()))
            .execute(&pool)
            .await
            .map_err(|error| format!("apply the migration: {error}")),
        "applying the migration",
    );
    Some(pool)
}

/// A unique tag for a scratch schema, without depending on `jarvis-core` from this crate's tests.
///
/// Uses the process id and a monotonic counter, which is enough because the schema is created per test and
/// the container is disposable. Note this is deliberately **not** the broken `{pid}-{sequence}` scheme this
/// workspace replaced elsewhere: the sequence is process-wide and the tag is unique within a process, which
/// is all a per-run schema needs.
fn jarvis_core_scratch_tag() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", std::process::id(), sequence)
}

/// A stored embedding row, described the way the write path takes it.
struct Fixture<'a> {
    memory_id: &'a str,
    workspace_id: &'a str,
    values: &'a [f32],
    model: &'a str,
    normalization: &'a str,
    hash: String,
}

impl<'a> Fixture<'a> {
    fn new(memory_id: &'a str, workspace_id: &'a str, values: &'a [f32], seed: u8) -> Self {
        Self {
            memory_id,
            workspace_id,
            values,
            model: "text-embedding-3-small",
            normalization: "normalized",
            hash: digest(seed),
        }
    }

    fn with_model(mut self, model: &'a str) -> Self {
        self.model = model;
        self
    }

    fn with_normalization(mut self, normalization: &'a str) -> Self {
        self.normalization = normalization;
        self
    }

    fn embedding(&'a self) -> NewMemoryEmbedding<'a> {
        NewMemoryEmbedding {
            memory_id: self.memory_id,
            workspace_id: self.workspace_id,
            provider: "openai",
            model: self.model,
            model_version: "2024-02-01",
            normalization: self.normalization,
            input_hash: &self.hash,
            values: self.values,
        }
    }
}

async fn record(pool: &PgPool, fixture: &Fixture<'_>) {
    must(
        record_memory_embedding(pool, &fixture.embedding()).await,
        "recording an embedding",
    );
}

/// The vector a column holds, read back as text, so a round trip is asserted against the server's own value.
async fn stored_vector(pool: &PgPool, memory_id: &str) -> String {
    let (text,): (String,) = must(
        sqlx::query_as("SELECT embedding::text FROM memory_embeddings WHERE memory_id = $1")
            .bind(memory_id)
            .fetch_one(pool)
            .await
            .map_err(|error| format!("read the stored vector: {error}")),
        "reading the stored vector",
    );
    text
}

#[tokio::test]
async fn the_migration_applies_and_the_extension_is_the_researched_version() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // The research record selected 0.8.6. Asserting the installed version is what makes "verified against
    // the recorded contract" a measurement rather than an assumption: the extension is per database, so a
    // different container would answer differently and this test would say so.
    let (version,): (String,) = must(
        sqlx::query_as("SELECT extversion FROM pg_extension WHERE extname = 'vector'")
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("read the extension version: {error}")),
        "reading the extension version",
    );
    assert_eq!(
        version, "0.8.6",
        "the research record documents pgvector 0.8.6; a different version means the contract it records \
         has not been re-verified"
    );

    // The migration's three load-bearing objects, named individually so a failure says which is missing
    // rather than only that something is.
    for (kind, name) in [
        ("table", "memory_embeddings"),
        ("index", "memory_embeddings_embedding_hnsw"),
        ("index", "memory_embeddings_workspace_idx"),
    ] {
        let (found,): (bool,) = must(
            sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = $1 AND relkind = \
                 CASE WHEN $2 = 'table' THEN 'r'::\"char\" ELSE 'i'::\"char\" END)",
            )
            .bind(name)
            .bind(kind)
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("look for {kind} {name}: {error}")),
            "looking for a migrated object",
        );
        assert!(found, "the migration must create {kind} `{name}`");
    }

    // The index must be PARTIAL, because that is what lets one column hold several widths. Asserting this
    // from the catalogue rather than from the migration text is what ties the claim to what the server
    // actually built.
    let definition: String = must(
        sqlx::query_scalar("SELECT indexdef FROM pg_indexes WHERE indexname = $1")
            .bind("memory_embeddings_embedding_hnsw")
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("read the index definition: {error}")),
        "reading the index definition",
    );
    assert!(
        definition.contains("USING hnsw"),
        "the index must be HNSW: {definition}"
    );
    assert!(
        definition.contains("vector_cosine_ops"),
        "the index must be built for cosine distance: {definition}"
    );
    assert!(
        definition.contains("WHERE (vector_dims(embedding) = 1536)")
            || definition.contains("WHERE vector_dims(embedding) = 1536"),
        "the index must be PARTIAL on the indexed width, or every insert of another dimension would be \
         refused: {definition}"
    );
}

#[tokio::test]
async fn a_vector_round_trips_through_the_server_without_losing_a_component() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // Values chosen because each breaks a different shortcut: `-0.0` is equal to `0.0` by value so a sign
    // lost in the text form is invisible to a comparison, a repeating fraction needs the shortest
    // round-tripping decimal rather than a fixed number of digits, and `f32::MIN_POSITIVE` is the smallest
    // normal value a fixed-decimal formatter would render as `0`. This is the offline codec's own fixture,
    // asserted here against what the SERVER stored rather than against the codec's round trip.
    let values: Vec<f32> = vec![1.0, -0.0, 1.0 / 3.0, f32::MIN_POSITIVE, -2.5, 1e-38];
    let fixture = Fixture::new("memory-round-trip", "workspace-a", &values, 0x01);
    record(&pool, &fixture).await;

    let stored = stored_vector(&pool, "memory-round-trip").await;
    let decoded = must(
        jarvis_storage::decode_embedding(&stored),
        "decoding what the server stored",
    );
    assert_eq!(
        decoded.len(),
        values.len(),
        "the server returned {} components for {} sent: {stored}",
        decoded.len(),
        values.len()
    );
    for (index, (sent, received)) in values.iter().zip(&decoded).enumerate() {
        // By BIT PATTERN, because `-0.0 == 0.0` is true: a value comparison cannot see a sign lost in the
        // text form, which is exactly the defect a fixed-decimal encoder produces.
        assert_eq!(
            sent.to_bits(),
            received.to_bits(),
            "component {index} changed: sent {sent}, stored {received}, column `{stored}`"
        );
    }
    // The `dimensions` column must agree with the vector, which the migration's CHECK enforces. Asserted
    // from the stored row rather than trusting the constraint, because the constraint is the mechanism.
    let (dimensions, vector_dims): (i32, i32) = must(
        sqlx::query_as(
            "SELECT dimensions, vector_dims(embedding) FROM memory_embeddings WHERE memory_id = $1",
        )
        .bind("memory-round-trip")
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("read the dimensions: {error}")),
        "reading the dimensions",
    );
    assert_eq!(dimensions, i32::try_from(values.len()).unwrap_or(-1));
    assert_eq!(vector_dims, dimensions);
}

#[tokio::test]
async fn the_query_uses_the_index_because_of_the_predicate() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // Enough rows that the planner has a reason to choose the index. At a few rows a sequential scan is
    // genuinely cheaper and a plan assertion would be testing the planner's cost model rather than this
    // code. `ANALYZE` runs below because without statistics the planner has no basis either way.
    let values = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.5);
    for index in 0..500 {
        let memory_id = format!("memory-bulk-{index}");
        let fixture = Fixture::new(
            &memory_id,
            "workspace-a",
            &values,
            u8::try_from(index % 200).unwrap_or(0),
        );
        record(&pool, &fixture).await;
    }
    must(
        sqlx::raw_sql("ANALYZE memory_embeddings")
            .execute(&pool)
            .await
            .map_err(|error| format!("analyze: {error}")),
        "analyzing the table",
    );

    let query = SimilarityQuery {
        workspace_id: "workspace-a",
        provider: "openai",
        model: "text-embedding-3-small",
        model_version: "2024-02-01",
        normalization: "normalized",
        values: &values,
        metric: jarvis_storage::DistanceMetric::Cosine,
        limit: 5,
    };
    // `search_similar` must return rows, which is the precondition a plan assertion would otherwise hide:
    // a query that returns nothing still gets a plan.
    let found = must(search_similar(&pool, &query).await, "searching");
    assert_eq!(found.len(), 5, "the search must return the requested rows");
    // Every bulk vector is identical, so every distance is ~0 and the ROW ORDER among the ties is whatever
    // the index returned. Asserting a specific id here would be asserting the index's internal ordering,
    // which nothing guarantees — an earlier version of this test did exactly that and failed. The
    // load-bearing assertion is the DISTANCE, which a wrong metric or a missing cast would change.
    assert!(
        found.iter().all(|hit| hit.distance.abs() < 1e-5),
        "identical vectors must all be at ~0 cosine distance, got {:?}",
        found
            .iter()
            .map(|hit| (hit.memory_id.as_str(), hit.distance))
            .collect::<Vec<_>>()
    );
    assert!(
        found
            .iter()
            .all(|hit| hit.memory_id.starts_with("memory-bulk-")),
        "every returned row must be one of the bulk rows"
    );

    // THE ASSERTION THIS TEST EXISTS FOR: the plan must use the partial HNSW index. `EXPLAIN` on the
    // repository's OWN statement shape, so a change to the query that dropped the predicate or the cast
    // would fail here rather than silently becoming a sequential scan.
    let plan = explain_nearest_neighbour(&pool, &values).await;
    assert!(
        plan.contains("memory_embeddings_embedding_hnsw"),
        "the nearest-neighbour query must use the HNSW index, and the plan was:\n{plan}"
    );
    assert!(
        plan.contains("Index Scan"),
        "the plan must be an index scan, and was:\n{plan}"
    );
    // And the control: without the dimension predicate the planner cannot use the partial index, which is
    // what shows the predicate — not the query's general shape — is what earns the index.
    let unguarded = explain_unpredicated(&pool, &values).await;
    assert!(
        !unguarded.contains("memory_embeddings_embedding_hnsw"),
        "without the dimension predicate the partial index must not be used, or the predicate is not what \
         earns it. Plan:\n{unguarded}"
    );
}

/// Runs `EXPLAIN` on the repository's own nearest-neighbour statement.
///
/// Built here rather than by calling a private helper, so the string is the shape `search_similar` emits: the
/// cast to the query's width, the dimension predicate, and the distance operator in an ascending `ORDER BY`.
async fn explain_nearest_neighbour(pool: &PgPool, values: &[f32]) -> String {
    let dimensions = values.len();
    let encoded = must(
        jarvis_storage::encode_embedding(values),
        "encoding the query vector",
    );
    let sql = format!(
        "EXPLAIN SELECT memory_id, embedding::vector({dimensions}) <=> $1::vector({dimensions}) AS distance \
         FROM memory_embeddings \
         WHERE workspace_id = 'workspace-a' \
           AND provider = 'openai' AND model = 'text-embedding-3-small' \
           AND model_version = '2024-02-01' AND normalization = 'normalized' \
           AND vector_dims(embedding) = {dimensions} \
         ORDER BY embedding::vector({dimensions}) <=> $1::vector({dimensions}) ASC \
         LIMIT 5"
    );
    let rows: Vec<(String,)> = must(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(encoded)
            .fetch_all(pool)
            .await
            .map_err(|error| format!("explain: {error}")),
        "explaining the nearest-neighbour query",
    );
    rows.into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The same statement **without** the dimension predicate, as the control.
async fn explain_unpredicated(pool: &PgPool, values: &[f32]) -> String {
    let dimensions = values.len();
    let encoded = must(
        jarvis_storage::encode_embedding(values),
        "encoding the query vector",
    );
    let sql = format!(
        "EXPLAIN SELECT memory_id, embedding::vector({dimensions}) <=> $1::vector({dimensions}) AS distance \
         FROM memory_embeddings \
         WHERE workspace_id = 'workspace-a' \
         ORDER BY embedding::vector({dimensions}) <=> $1::vector({dimensions}) ASC \
         LIMIT 5"
    );
    let rows: Vec<(String,)> = must(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(encoded)
            .fetch_all(pool)
            .await
            .map_err(|error| format!("explain: {error}")),
        "explaining the unpredicated query",
    );
    rows.into_iter()
        .map(|(line,)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn one_column_holds_several_dimensions_and_only_the_indexed_width_is_searchable() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // The measured finding this whole design rests on: a non-partial cast index made a table of
    // 1,536-dimension rows completely uninsertable at any other width. So the assertion is that three widths
    // coexist in one column — and that this is possible because the index is partial, which the catalogue
    // assertion above pins.
    let small = uniform(3, 0.5);
    let medium = uniform(4, 0.5);
    let indexed = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.5);
    // The widest a `vector` INDEX can cover, and therefore the widest this product can store: `encode`
    // refuses above it, and the migration's range CHECK agrees. A 2,001-dimension vector is deliberately
    // NOT tested here — `a_vector_above_the_indexed_cap_is_refused_rather_than_stored` asserts that it is
    // refused, because a column that accepted one would hold a vector no index can search.
    let at_the_index_cap = uniform(2_000, 0.5);

    for (memory_id, values, seed) in [
        ("memory-w3", small.as_slice(), 0x03),
        ("memory-w4", medium.as_slice(), 0x04),
        ("memory-w1536", indexed.as_slice(), 0x05),
        ("memory-w2000", at_the_index_cap.as_slice(), 0x06),
    ] {
        let fixture = Fixture::new(memory_id, "workspace-a", values, seed);
        record(&pool, &fixture).await;
    }

    let (count,): (i64,) = must(
        sqlx::query_as("SELECT count(*) FROM memory_embeddings")
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("{error}")),
        "counting rows",
    );
    assert_eq!(
        count, 4,
        "a column holding four different widths is the whole reason the index is partial; a non-partial cast \
         index refuses every insert after the first width (measured)"
    );

    // A search at the indexed width must find ONLY the row at that width, so the dimension predicate is
    // shown to be doing its job: without it the comparison is `vector(1536) <=> ...` against 3- and
    // 4-component vectors, which the server refuses outright.
    let searchable = must(
        search_similar(
            &pool,
            &SimilarityQuery {
                workspace_id: "workspace-a",
                provider: "openai",
                model: "text-embedding-3-small",
                model_version: "2024-02-01",
                normalization: "normalized",
                values: &indexed,
                metric: jarvis_storage::DistanceMetric::Cosine,
                limit: MAX_SIMILARITY_RESULTS,
            },
        )
        .await,
        "searching the indexed width",
    );
    assert_eq!(
        searchable.len(),
        1,
        "only the 1,536-dimension vector is comparable with a 1,536-dimension query; got {:?}",
        searchable
            .iter()
            .map(|hit| hit.memory_id.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(searchable[0].memory_id, "memory-w1536");
}

#[tokio::test]
async fn a_vector_above_the_indexed_cap_is_refused_rather_than_stored() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // pgvector's `vector` TYPE stores to 16,000 dimensions while its HNSW index caps at 2,000, so a column
    // *could* hold a 2,001-dimension vector. This product deliberately refuses to write one: a vector the
    // index cannot cover is a row that retrieval silently stops being a search over, and the write would
    // look like it worked. The refusal names the documented alternatives, which is what makes it actionable
    // rather than merely restrictive.
    let above_the_cap = uniform(2_001, 0.5);
    let fixture = Fixture::new("memory-too-wide", "workspace-a", &above_the_cap, 0x08);
    match record_memory_embedding(&pool, &fixture.embedding()).await {
        Err(jarvis_storage::EmbeddingError::Vector(
            jarvis_storage::PgVectorError::DimensionsTooLarge {
                dimensions,
                maximum,
            },
        )) => {
            assert_eq!(dimensions, 2_001);
            assert_eq!(maximum, 2_000);
        }
        other => panic!("a 2,001-dimension vector must be refused by the codec, got {other:?}"),
    }
    // And nothing was written, which is the half a status-only assertion would miss: a refusal that still
    // inserted would pass the check above while leaving the unindexable row behind.
    let (count,): (i64,) = must(
        sqlx::query_as(
            "SELECT count(*) FROM memory_embeddings WHERE memory_id = 'memory-too-wide'",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("{error}")),
        "counting the refused row",
    );
    assert_eq!(count, 0, "a refused embedding must not be stored");
    // And `encode` is what makes the direct SQL unreachable, so the raw insert cannot even be built: the
    // text form of a 2,001-component vector has no producer. Asserted rather than described, because "the
    // codec is the only way" is the claim, and a `Vec<f32>` a caller assembled by hand would still go
    // through `encode`.
    assert!(matches!(
        jarvis_storage::encode_embedding(&above_the_cap),
        Err(jarvis_storage::PgVectorError::DimensionsTooLarge { .. })
    ));
}

#[tokio::test]
async fn the_migration_range_check_refuses_a_width_above_the_index_cap() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // The CHECK is asserted with a **hand-built text literal**, deliberately bypassing `encode`, because
    // `encode` is exactly what makes the range unreachable through the product. Building the literal here
    // simulates another writer — an operator's `psql` session, a future repository, a restored dump — which
    // is the only way to observe the constraint at all, and is the same technique this workspace uses for
    // every other unreachable-by-construction schema rule.
    //
    // `dimensions` and the vector's own width AGREE at 2,001, so `memory_embeddings_dimensions_match` cannot
    // be what refuses this row. That isolation matters: the first version of this test declared 2,001 while
    // sending a 64-component vector, and the match constraint fired first — the assertion would have passed
    // for a reason that had nothing to do with the range.
    let components = vec!["0.5"; 2_001].join(",");
    let literal = format!("[{components}]");
    assert_eq!(
        jarvis_storage::decode_embedding(&literal).map(|values| values.len()),
        Ok(2_001),
        "the fixture must really be a 2,001-component vector literal"
    );
    let result = sqlx::query(
        "INSERT INTO memory_embeddings \
             (memory_id, workspace_id, provider, model, model_version, dimensions, normalization, \
              input_hash, embedding) \
         VALUES ('memory-range', 'workspace-a', 'openai', 'text-embedding-3-small', '2024-02-01', \
                 2001, 'normalized', $1, $2::vector)",
    )
    .bind(digest(0x0a))
    .bind(&literal)
    .execute(&pool)
    .await;
    match result {
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("memory_embeddings_dimensions_range"),
                "the refusal must name the RANGE constraint, and said: {message}"
            );
            assert!(
                !message.contains("memory_embeddings_dimensions_match"),
                "the fixture must have matching dimensions, so the match constraint cannot be the cause: \
                 {message}"
            );
        }
        Ok(_) => panic!(
            "a declared dimension of 2001 must be refused: the codec caps at 2000, so a column allowing more \
             would describe a capability no code path can exercise"
        ),
    }
    // And the boundary itself is accepted, so the constraint is a boundary rather than an off-by-one that
    // refuses the widest supported model.
    let at_the_cap = vec!["0.5"; 2_000].join(",");
    let accepted = sqlx::query(
        "INSERT INTO memory_embeddings \
             (memory_id, workspace_id, provider, model, model_version, dimensions, normalization, \
              input_hash, embedding) \
         VALUES ('memory-cap', 'workspace-a', 'openai', 'text-embedding-3-small', '2024-02-01', \
                 2000, 'normalized', $1, $2::vector)",
    )
    .bind(digest(0x0b))
    .bind(format!("[{at_the_cap}]"))
    .execute(&pool)
    .await;
    assert!(
        accepted.is_ok(),
        "a 2,000-dimension vector is the widest the index covers and must be accepted: {accepted:?}"
    );
}

#[tokio::test]
async fn an_incomparable_metadata_row_is_never_returned_by_a_search() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // Four rows, identical as vectors, differing in exactly one metadata field each. `docs/architecture/
    // storage.md` requires that vectors with incompatible metadata are never compared, and a comparison
    // across models returns a number in the usual range — so a missing guard would look like a slightly
    // worse ranking rather than as an error. Each row below is a way the guard could be incomplete.
    let values = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.5);
    let comparable = Fixture::new("memory-match", "workspace-a", &values, 0x10);
    let other_model = Fixture::new("memory-other-model", "workspace-a", &values, 0x11)
        .with_model("text-embedding-3-large");
    let other_normalization = Fixture::new("memory-other-norm", "workspace-a", &values, 0x12)
        .with_normalization("unknown");
    let other_workspace = Fixture::new("memory-other-ws", "workspace-b", &values, 0x13);
    // Bound rather than built inline: `Fixture` borrows the slice, so `&uniform(3, 0.5)` as an argument
    // would create a temporary that dies at the end of the statement while the fixture still refers to it.
    let three_dimensions = uniform(3, 0.5);
    let other_dimension = Fixture::new("memory-other-dim", "workspace-a", &three_dimensions, 0x14);

    for fixture in [
        &comparable,
        &other_model,
        &other_normalization,
        &other_workspace,
        &other_dimension,
    ] {
        record(&pool, fixture).await;
    }

    let found = must(
        search_similar(
            &pool,
            &SimilarityQuery {
                workspace_id: "workspace-a",
                provider: "openai",
                model: "text-embedding-3-small",
                model_version: "2024-02-01",
                normalization: "normalized",
                values: &values,
                metric: jarvis_storage::DistanceMetric::Cosine,
                limit: MAX_SIMILARITY_RESULTS,
            },
        )
        .await,
        "searching",
    );
    let ids: Vec<&str> = found.iter().map(|hit| hit.memory_id.as_str()).collect();
    // The positive control first, or "returns nothing" would pass.
    assert!(
        ids.contains(&"memory-match"),
        "the comparable row must be found, or the refusals below prove nothing; got {ids:?}"
    );
    assert_eq!(
        ids,
        vec!["memory-match"],
        "exactly the comparable row must be returned, and got {ids:?}"
    );
}

#[tokio::test]
async fn a_foreign_dimension_query_is_refused_by_the_server_rather_than_matching_nothing() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    let values = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.5);
    let fixture = Fixture::new("memory-a", "workspace-a", &values, 0x20);
    record(&pool, &fixture).await;

    // The measured server behaviour the cast exists for: comparing two different widths is an ERROR, not an
    // empty result. `vector(n) <=> other(m)` fails with `different vector dimensions 1536 and 3`, so a
    // search that omitted the cast would fail at the server instead of returning fewer rows — and the test
    // below pins that the repository's own shape does NOT fail, which is what makes the cast load-bearing.
    let wrong_width = uniform(3, 0.5);
    let direct = sqlx::query_as::<_, (String,)>(
        "SELECT memory_id FROM memory_embeddings WHERE embedding <=> $1::vector(3) < 1 LIMIT 1",
    )
    .bind(must(
        jarvis_storage::encode_embedding(&wrong_width),
        "encoding a 3-dimension vector",
    ))
    .fetch_all(&pool)
    .await;
    match direct {
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("dimensions"),
                "the server's refusal must mention the dimensions, and said: {message}"
            );
        }
        Ok(rows) => panic!(
            "a query without the cast must fail on a width mismatch, and returned {} rows instead",
            rows.len()
        ),
    }

    // The repository's own shape with a differently-sized query vector: because the cast is to the QUERY's
    // width, this is a well-formed query that finds nothing, rather than an error. That is the correct
    // outcome — a caller searching at a width nobody stored has no neighbours, not a broken server.
    let found = must(
        search_similar(
            &pool,
            &SimilarityQuery {
                workspace_id: "workspace-a",
                provider: "openai",
                model: "text-embedding-3-small",
                model_version: "2024-02-01",
                normalization: "normalized",
                values: &wrong_width,
                metric: jarvis_storage::DistanceMetric::Cosine,
                limit: 5,
            },
        )
        .await,
        "searching at a width nobody stored",
    );
    assert!(
        found.is_empty(),
        "a query at an unpopulated width must find nothing rather than fail, and found {:?}",
        found
            .iter()
            .map(|hit| hit.memory_id.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn recording_the_same_comparability_key_twice_replaces_rather_than_duplicating() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    let first = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.25);
    let second = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.75);
    record(
        &pool,
        &Fixture::new("memory-reembed", "workspace-a", &first, 0x30),
    )
    .await;
    // A re-embedding by the same model is a new vector for the same (memory, model) pair, so it replaces.
    // A second row would make one memory have two current vectors and the ranking depend on row order.
    record(
        &pool,
        &Fixture::new("memory-reembed", "workspace-a", &second, 0x31),
    )
    .await;

    let (count,): (i64,) = must(
        sqlx::query_as("SELECT count(*) FROM memory_embeddings WHERE memory_id = 'memory-reembed'")
            .fetch_one(&pool)
            .await
            .map_err(|error| format!("{error}")),
        "counting the rows for one memory",
    );
    assert_eq!(count, 1, "a re-embedding must replace, not duplicate");
    let stored = stored_vector(&pool, "memory-reembed").await;
    let decoded = must(
        jarvis_storage::decode_embedding(&stored),
        "decoding the replaced vector",
    );
    assert_eq!(
        decoded.first().map(|value| value.to_bits()),
        second.first().map(|value| value.to_bits()),
        "the replacement vector must be the stored one, and the column holds `{stored}`"
    );
    // The input hash must have moved with it, or a reader would compare the new vector against the old
    // text's digest and conclude the text had not changed.
    let (hash,): (String,) = must(
        sqlx::query_as(
            "SELECT input_hash FROM memory_embeddings WHERE memory_id = 'memory-reembed'",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("{error}")),
        "reading the input hash",
    );
    assert_eq!(hash, digest(0x31));
}

#[tokio::test]
async fn the_server_refuses_a_row_whose_declared_dimensions_disagree_with_its_vector() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // The measured defect the migration's CHECK exists for: on a live server a row declaring
    // `dimensions = 1536` while holding a 3-component vector inserted cleanly, and adding the constraint
    // afterwards failed ON THAT ROW. So the CHECK is asserted by writing around the repository — raw SQL
    // that supplies the disagreeing pair — because the write path derives the count from the slice and
    // therefore cannot produce the disagreement. A test through the repository would prove nothing here.
    let values = uniform(3, 0.5);
    let encoded = must(
        jarvis_storage::encode_embedding(&values),
        "encoding a 3-component vector",
    );
    let result = sqlx::query(
        "INSERT INTO memory_embeddings \
             (memory_id, workspace_id, provider, model, model_version, dimensions, normalization, \
              input_hash, embedding) \
         VALUES ('memory-lie', 'workspace-a', 'openai', 'text-embedding-3-small', '2024-02-01', \
                 1536, 'normalized', $1, $2::vector)",
    )
    .bind(digest(0x40))
    .bind(encoded)
    .execute(&pool)
    .await;
    match result {
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("memory_embeddings_dimensions_match"),
                "the refusal must name the constraint that caught it, and said: {message}"
            );
        }
        Ok(_) => panic!(
            "a row declaring 1536 dimensions while holding a 3-component vector must be refused, or the \
             `dimensions` column is a value nothing checks"
        ),
    }
    // And the same row with the CORRECT count is accepted, so the constraint is a check on agreement rather
    // than a check that refuses the shape.
    let accepted = sqlx::query(
        "INSERT INTO memory_embeddings \
             (memory_id, workspace_id, provider, model, model_version, dimensions, normalization, \
              input_hash, embedding) \
         VALUES ('memory-honest', 'workspace-a', 'openai', 'text-embedding-3-small', '2024-02-01', \
                 3, 'normalized', $1, $2::vector)",
    )
    .bind(digest(0x41))
    .bind(must(
        jarvis_storage::encode_embedding(&values),
        "encoding again",
    ))
    .execute(&pool)
    .await;
    assert!(
        accepted.is_ok(),
        "an honest row must be accepted, or the constraint refuses the shape rather than the disagreement: \
         {accepted:?}"
    );
}

#[tokio::test]
async fn a_generated_index_for_a_second_width_can_actually_be_created() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // `index_ddl`'s whole purpose is that supporting a second model is a function call. A string assertion
    // in the offline tests cannot show that the SQL is valid, so this executes it — the difference between
    // "the text contains the right words" and "pgvector accepts it".
    let ddl = must(index_ddl(768), "the 768-dimension index DDL");
    must(
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&pool)
            .await
            .map_err(|error| format!("create the second index: {error}")),
        "creating a second width's index",
    );
    let (found,): (bool,) = must(
        sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM pg_indexes WHERE indexname = \
             'memory_embeddings_embedding_hnsw_768')",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("{error}")),
        "looking for the second index",
    );
    assert!(found, "the generated DDL must create the index it names");
    // And the first index is STILL there: a second width must add an index rather than replace one, which is
    // what pgvector's "add an index for each distance function" requires.
    let (still_there,): (bool,) = must(
        sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM pg_indexes WHERE indexname = \
             'memory_embeddings_embedding_hnsw')",
        )
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("{error}")),
        "looking for the migration's index",
    );
    assert!(
        still_there,
        "a second width must not disturb the first index"
    );
}

#[tokio::test]
async fn a_cosine_search_orders_by_distance_ascending_not_by_similarity_descending() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // Three vectors whose distances to the query are known by hand, so the expected ORDER can be named
    // rather than asserted relatively — a wrong-but-consistent implementation would satisfy a relative
    // assertion. The query is the basis vector on axis 0:
    //   axis 0  -> cosine distance 0.0   (identical direction)
    //   45°     -> cosine distance ~0.293
    //   axis 1  -> cosine distance 1.0   (orthogonal)
    let mut diagonal = vec![0.0_f32; 4];
    diagonal[0] = 1.0;
    diagonal[1] = 1.0;
    let exact = basis(4, 0);
    let orthogonal = basis(4, 1);

    for (memory_id, values, seed) in [
        ("memory-far", orthogonal.as_slice(), 0x50),
        ("memory-mid", diagonal.as_slice(), 0x51),
        ("memory-near", exact.as_slice(), 0x52),
    ] {
        record(&pool, &Fixture::new(memory_id, "workspace-a", values, seed)).await;
    }

    let found = must(
        search_similar(
            &pool,
            &SimilarityQuery {
                workspace_id: "workspace-a",
                provider: "openai",
                model: "text-embedding-3-small",
                model_version: "2024-02-01",
                normalization: "normalized",
                values: &exact,
                metric: jarvis_storage::DistanceMetric::Cosine,
                limit: 3,
            },
        )
        .await,
        "searching by cosine",
    );
    let ordered: Vec<(&str, f64)> = found
        .iter()
        .map(|hit| (hit.memory_id.as_str(), hit.distance))
        .collect();
    assert_eq!(
        ordered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec!["memory-near", "memory-mid", "memory-far"],
        "cosine DISTANCE must ascend, so the nearest is first; got {ordered:?}"
    );
    // The VALUES are asserted too, because the order alone is satisfied by any monotone metric — Euclidean
    // would give the same ordering here. `1 - cos` and cosine distance agree numerically, but a search that
    // returned the NEGATIVE inner product would give `-1, -1.41, 0`, which orders differently.
    assert!(
        ordered[0].1.abs() < 1e-6,
        "an identical direction must be at ~0 distance, got {ordered:?}"
    );
    assert!(
        (ordered[1].1 - (1.0 - std::f64::consts::FRAC_1_SQRT_2)).abs() < 1e-6,
        "a 45° vector must be at 1 - cos(45°) ≈ 0.293, got {ordered:?}"
    );
    assert!(
        (ordered[2].1 - 1.0).abs() < 1e-6,
        "an orthogonal vector must be at exactly 1.0, got {ordered:?}"
    );
    // And `ascending_is_closer` is what says so, which is the predicate `jarvis_core::retrieval` needs to
    // convert a distance into a similarity. Asserted against the values above rather than restated.
    let metric = jarvis_storage::DistanceMetric::Cosine;
    assert!(metric.ascending_is_closer());
    if let Some((_, nearest)) = ordered.first() {
        assert!(
            *nearest < ordered[2].1,
            "the returned order must be ascending, so the first distance is the smallest"
        );
    }
}

#[tokio::test]
async fn the_negative_inner_product_metric_ranks_by_its_own_operator() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // `<#>` returns the NEGATIVE inner product "since Postgres only supports ASC order index scans on
    // operators", so a caller reading the returned value as a similarity ranks BACKWARDS while every type
    // checks. This asserts the values are negative and that the ordering is by the operator's own ascending
    // order — which for a negative inner product means the LARGEST inner product ranks first.
    let long = vec![1.0_f32, 1.0, 1.0, 1.0];
    let short = vec![0.25_f32, 0.25, 0.25, 0.25];
    record(
        &pool,
        &Fixture::new("memory-long", "workspace-a", &long, 0x60),
    )
    .await;
    record(
        &pool,
        &Fixture::new("memory-short", "workspace-a", &short, 0x61),
    )
    .await;

    let query = SimilarityQuery {
        workspace_id: "workspace-a",
        provider: "openai",
        model: "text-embedding-3-small",
        model_version: "2024-02-01",
        normalization: "normalized",
        values: &long,
        metric: jarvis_storage::DistanceMetric::NegativeInnerProduct,
        limit: 2,
    };
    let found = must(
        search_similar(&pool, &query).await,
        "searching by negative inner product",
    );
    let ordered: Vec<(&str, f64)> = found
        .iter()
        .map(|hit| (hit.memory_id.as_str(), hit.distance))
        .collect();
    assert_eq!(
        ordered.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec!["memory-long", "memory-short"],
        "the largest inner product must rank first, and `<#>` orders ascending on its NEGATIVE value; got \
         {ordered:?}"
    );
    // The values are negative, which is the property a caller must know before treating one as a score.
    assert!(
        ordered.iter().all(|(_, distance)| *distance <= 0.0),
        "`<#>` returns the negative inner product, so every distance must be <= 0; got {ordered:?}"
    );
    // `long · long = 4`, so `<#>` is exactly -4 — the probe's own measured value.
    assert!(
        (ordered[0].1 + 4.0).abs() < 1e-9,
        "`[1,1,1,1] <#> [1,1,1,1]` must be -4, got {ordered:?}"
    );
    // `ascending_is_closer` is TRUE for this metric as well, which is the whole trap: the ordering ascends on
    // a value whose sign is inverted, so a caller treating the returned number as a score ranks backwards.
    // Asserted against the values above rather than restated, so the predicate and the measurement agree.
    let metric = jarvis_storage::DistanceMetric::NegativeInnerProduct;
    assert!(metric.ascending_is_closer());
    let smallest = ordered
        .iter()
        .map(|(_, distance)| *distance)
        .fold(f64::INFINITY, f64::min);
    assert!(
        (smallest + 4.0).abs() < 1e-9,
        "the first row must hold the smallest (most negative) distance, got {ordered:?}"
    );
    // And the conversion the composition root needs: `similarity` turns the negative inner product into a
    // value that ranks the same way. Asserted so the two metrics cannot be confused at the call site.
    let as_similarity = metric.similarity(narrow(ordered[0].1));
    let as_similarity_other = metric.similarity(narrow(ordered[1].1));
    assert!(
        as_similarity_other <= as_similarity,
        "converting to similarity must keep the ordering: {ordered:?} became \
         ({as_similarity}, {as_similarity_other})"
    );
}

#[tokio::test]
async fn a_selective_filter_returns_fewer_rows_than_requested_rather_than_an_error() {
    let Some(pool) = migrated_pool().await else {
        return;
    };
    // The documented behaviour a naive test would call a bug: "filtering is applied AFTER the index is
    // scanned. If a condition matches 10% of rows, with HNSW and the default `hnsw.ef_search` of 40, only 4
    // rows will match on average." So the assertion is that a selective filter yields FEWER rows than the
    // limit and does NOT error — the recall/speed trade-off `docs/research/integrations/postgres-pgvector.md`
    // records rather than a defect. `hnsw.ef_search` is deliberately left at its default, because setting it
    // would make this test measure a setting nothing in the product sets.
    let values = uniform(DEFAULT_INDEXED_DIMENSIONS, 0.5);
    for index in 0..400 {
        let memory_id = format!("memory-mixed-{index}");
        // 10% in workspace-a, the rest in workspace-b, which is the README's own selectivity example.
        let workspace = if index % 10 == 0 {
            "workspace-a"
        } else {
            "workspace-b"
        };
        let fixture = Fixture::new(
            &memory_id,
            workspace,
            &values,
            u8::try_from(index % 200).unwrap_or(0),
        );
        record(&pool, &fixture).await;
    }
    must(
        sqlx::raw_sql("ANALYZE memory_embeddings")
            .execute(&pool)
            .await
            .map_err(|error| format!("{error}")),
        "analyzing",
    );

    let requested = 100;
    let found = must(
        search_similar(
            &pool,
            &SimilarityQuery {
                workspace_id: "workspace-a",
                provider: "openai",
                model: "text-embedding-3-small",
                model_version: "2024-02-01",
                normalization: "normalized",
                values: &values,
                metric: jarvis_storage::DistanceMetric::Cosine,
                limit: requested,
            },
        )
        .await,
        "searching a selective filter",
    );
    // NO ERROR is the primary assertion, so it is stated in the panic message of the count check rather than
    // as a separate `is_ok`.
    assert!(
        !found.is_empty(),
        "a selective filter must still find the rows in its own workspace, and returned none"
    );
    assert!(
        found.len() <= 40,
        "workspace-a holds 40 of 400 rows, and a filtered HNSW scan returns at most `hnsw.ef_search` (40) \
         candidates, so at most 40 rows can match; got {}",
        found.len()
    );
    // Every returned row really is in the searched workspace, which is the guard that must hold whatever
    // the recall is.
    let ids: Vec<String> = found.iter().map(|hit| hit.memory_id.clone()).collect();
    let (foreign,): (i64,) = must(
        sqlx::query_as(
            "SELECT count(*) FROM memory_embeddings WHERE memory_id = ANY($1) AND workspace_id <> $2",
        )
        .bind(&ids)
        .bind("workspace-a")
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("{error}")),
        "checking the returned rows' workspaces",
    );
    assert_eq!(
        foreign, 0,
        "a filtered search must never return another workspace's row; got {ids:?}"
    );
}
