//! Tests for the embedding repository's off-line half.
//!
//! The live-server half lives in `tests/postgres_embeddings.rs`, gated by `ACCEPTANCE_POSTGRES_URL`, because
//! a test that needs a database must not be the reason a checkout without one fails — and because a test
//! that *silently* skipped would let a claim about a server rest on nothing. See that file's module doc for
//! the guard, which is falsified in the same way as the other acceptance guards in this workspace.
//!
//! What is here is everything decidable without a server: the DDL the migration and [`index_ddl`] must agree
//! on, the refusals a caller can act on, and the metadata guard's shape. These are the assertions that would
//! otherwise be made against the live tests' setup, where a failure would look like a database problem.

use super::*;

/// The migration's text, read from the crate's own migrations directory.
///
/// Read rather than restated, because the point of these tests is that the migration and `index_ddl` agree.
/// A copy of the SQL would let both drift together and keep passing.
const MIGRATION: &str = include_str!("../migrations/postgres/0001_memory_embeddings.sql");

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// A vector of `count` identical components, which is a valid embedding and keeps the assertions readable.
fn uniform(count: usize, value: f32) -> Vec<f32> {
    vec![value; count]
}

/// A 64-character lowercase hex digest, built rather than typed so its length cannot drift.
fn digest(seed: u8) -> String {
    let hex = format!("{seed:02x}");
    hex.repeat(32)
}

fn embedding<'a>(values: &'a [f32], hash: &'a str) -> NewMemoryEmbedding<'a> {
    NewMemoryEmbedding {
        memory_id: "memory-1",
        workspace_id: "workspace-1",
        provider: "openai",
        model: "text-embedding-3-small",
        model_version: "2024-02-01",
        normalization: "normalized",
        input_hash: hash,
        values,
    }
}

#[test]
fn the_migration_and_the_ddl_helper_describe_the_same_index() {
    // The load-bearing parts of the index are the cast width and the dimension predicate. Both must be
    // present in the migration, because the live tests assert an `Index Scan` that only happens when the
    // predicate is stated in the index AND in the query.
    assert!(
        MIGRATION.contains("USING hnsw"),
        "the migration must create an HNSW index"
    );
    assert!(
        MIGRATION.contains("(embedding::vector(1536)) vector_cosine_ops"),
        "the migration must index the cast expression"
    );
    assert!(
        MIGRATION.contains("WHERE vector_dims(embedding) = 1536"),
        "the migration's index must be PARTIAL: without the predicate, every insert of another dimension is \
         refused (measured), so the predicate is what makes one column hold several widths"
    );
    // And `index_ddl` must produce the same three properties for the migration's own width.
    let generated = must(
        index_ddl(DEFAULT_INDEXED_DIMENSIONS),
        "the default index DDL",
    );
    assert!(
        generated.contains("USING hnsw"),
        "the generated DDL must create an HNSW index: {generated}"
    );
    assert!(
        generated.contains("(embedding::vector(1536)) vector_cosine_ops"),
        "the generated DDL must index the cast expression: {generated}"
    );
    assert!(
        generated.contains("WHERE vector_dims(embedding) = 1536"),
        "the generated DDL must be partial, or it would break every other width: {generated}"
    );
    // The migration's own index name must be the generated one for the default width, so a deployment that
    // runs the migration and a deployment that calls this helper do not end up with two indexes.
    assert!(
        generated.contains("memory_embeddings_embedding_hnsw_1536"),
        "got {generated}"
    );
}

#[test]
fn the_migration_ties_the_dimension_column_to_the_vector() {
    // The measured defect: a row declaring `dimensions = 1536` while holding a 3-component vector inserted
    // cleanly, because nothing compared the two. A reader using `dimensions` to decide comparability would
    // then be reading a column that can lie.
    assert!(
        MIGRATION.contains("CHECK (vector_dims(embedding) = dimensions)"),
        "the migration must tie the declared dimensions to the vector's own length"
    );
    // The range bound is the **index** cap, not pgvector's 16,000-dimension storage cap. `encode` refuses to
    // write a vector the index cannot cover, so a bound of 16,000 would describe a capability no code path
    // can exercise — and a direct insert that used the room would create a row retrieval silently stops
    // being a search over. This assertion is what keeps the schema and the codec's cap from drifting apart.
    assert!(
        MIGRATION.contains("BETWEEN 1 AND 2000"),
        "the dimension bound must be the index cap, which is the widest vector the codec will write"
    );
    // The bound must not be pgvector's 16,000-dimension storage cap. Asserted as the **constraint clause**
    // rather than as the absence of the number, because the comment above the constraint explains why 16,000
    // is rejected and naming it there is the documentation, not the bound.
    assert!(
        !MIGRATION.contains("BETWEEN 1 AND 16000"),
        "the migration must not bound the column by pgvector's storage cap: it is above what this product \
         can index, so allowing it would be a claim nothing can satisfy"
    );
    // The extension statement must be inside the migration, and idempotent, because a `vector` column in a
    // database without the extension fails as `type \"vector\" does not exist` rather than as a missing
    // extension.
    assert!(
        MIGRATION.contains("CREATE EXTENSION IF NOT EXISTS vector"),
        "the migration must install the extension idempotently"
    );
}

#[test]
fn a_generated_index_refuses_a_width_pgvector_would_reject() {
    // pgvector's own error, measured: `column cannot have more than 2000 dimensions for hnsw index`. The
    // refusal happens here so the message names the cap rather than sending the reader to the server's.
    let at_the_cap = must(index_ddl(pgvector::MAX_INDEXED_DIMENSIONS), "the cap");
    assert!(at_the_cap.contains("vector(2000)"), "got {at_the_cap}");

    for bad in [0, pgvector::MAX_INDEXED_DIMENSIONS + 1, 16_000, 16_384] {
        match index_ddl(bad) {
            Err(EmbeddingError::ResultLimit { requested, maximum }) => {
                assert_eq!(requested, bad);
                assert_eq!(maximum, pgvector::MAX_INDEXED_DIMENSIONS);
                // The message must name the bound, because "an index above 2,000 is impossible" is the
                // actionable content.
                let message = EmbeddingError::ResultLimit {
                    requested: bad,
                    maximum,
                }
                .to_string();
                assert!(message.contains("2000"), "got {message}");
            }
            other => panic!("{bad} dimensions must be refused, got {other:?}"),
        }
    }
}

#[test]
fn a_second_width_gets_its_own_index_rather_than_replacing_the_first() {
    // A different model is a second width, and pgvector indexes one width per index. The generated names
    // must therefore differ, or the second model's index would replace the first's and silently stop
    // covering the first model's vectors.
    //
    // 768 rather than 3,072: pgvector refuses an HNSW index above 2,000 dimensions, so a 3,072-dimension
    // model is a case `index_ddl` correctly rejects — which is asserted separately below rather than used
    // as the "second width".
    let first = must(index_ddl(1_536), "1536");
    let second = must(index_ddl(768), "768");
    assert_ne!(first, second);
    assert!(
        first.contains("memory_embeddings_embedding_hnsw_1536"),
        "got {first}"
    );
    assert!(
        second.contains("memory_embeddings_embedding_hnsw_768"),
        "got {second}"
    );
    assert!(
        second.contains("vector_dims(embedding) = 768"),
        "each width's index must be partial on its own width: {second}"
    );
    // And `IF NOT EXISTS`, so running the same migration twice is a no-op rather than an error.
    assert!(first.contains("IF NOT EXISTS"), "got {first}");
    // A width pgvector cannot index is refused rather than emitted as DDL the server would reject with a
    // message that does not name this crate. 3,072 is the researched provider's large model, so this is a
    // real model rather than a hypothetical.
    assert!(matches!(
        index_ddl(3_072),
        Err(EmbeddingError::ResultLimit { .. })
    ));
}

#[test]
fn a_search_refuses_a_limit_it_cannot_honour() {
    // Zero is refused rather than read as "no limit": from an empty result a caller cannot tell "nothing is
    // similar" from "you asked for none", and those lead to opposite next steps. The guard function is
    // called, so the boundary asserted here is the boundary that runs.
    for bad in [0, MAX_SIMILARITY_RESULTS + 1, usize::MAX] {
        match check_result_limit(bad) {
            Err(EmbeddingError::ResultLimit { requested, maximum }) => {
                assert_eq!(requested, bad);
                assert_eq!(maximum, MAX_SIMILARITY_RESULTS);
                let message = EmbeddingError::ResultLimit {
                    requested: bad,
                    maximum,
                }
                .to_string();
                assert!(
                    message.contains(&MAX_SIMILARITY_RESULTS.to_string()),
                    "the refusal must name the bound: {message}"
                );
            }
            other => panic!("a limit of {bad} must be refused, got {other:?}"),
        }
    }
    // The bound's own value is accepted, so the refusal is a boundary and not an off-by-one that refuses
    // everything — and one is accepted, so the low end is not accidentally closed.
    assert!(check_result_limit(MAX_SIMILARITY_RESULTS).is_ok());
    assert!(check_result_limit(1).is_ok());
}

#[test]
fn a_metadata_field_that_is_empty_is_refused_and_the_field_is_named() {
    let values = uniform(4, 0.5);
    let hash = digest(0xab);
    // The metadata decides comparability, so an empty provider makes two incomparable vectors look
    // comparable. The guard itself is called, not reimplemented: a test that restated the predicate could
    // agree with itself while `check_metadata` was wrong.
    assert!(check_metadata(&embedding(&values, &hash)).is_ok());
    for (mutated, field) in [
        ("memory_id", "memory_id"),
        ("workspace_id", "workspace_id"),
        ("provider", "provider"),
        ("model", "model"),
        ("model_version", "model_version"),
        ("normalization", "normalization"),
    ] {
        let mut candidate = embedding(&values, &hash);
        // Whitespace counts as empty as well, because a provider that stored `" "` would satisfy the
        // schema's NOT NULL while meaning nothing.
        match mutated {
            "memory_id" => candidate.memory_id = "   ",
            "workspace_id" => candidate.workspace_id = "",
            "provider" => candidate.provider = " ",
            "model" => candidate.model = "",
            "model_version" => candidate.model_version = "  ",
            "normalization" => candidate.normalization = "",
            _ => unreachable!(),
        }
        match check_metadata(&candidate) {
            Err(EmbeddingError::MetadataEmpty { field: named }) => {
                assert_eq!(named, field, "the refusal must name the offending field");
            }
            other => panic!("an empty `{field}` must be refused, got {other:?}"),
        }
    }
}

#[test]
fn both_dimension_sites_agree_on_the_binding() {
    // The write path and the search both bind a width that must equal `vector_dims(embedding)` in the row or
    // the query's predicate. They share `dimension_count`, so this pins the shared behaviour rather than two
    // copies of it: a value no `i64` holds is refused, and an ordinary one converts unchanged.
    assert_eq!(must(dimension_count(1_536), "1536 dimensions"), 1_536_i64);
    assert_eq!(must(dimension_count(1), "one dimension"), 1_i64);
    assert_eq!(
        must(
            dimension_count(pgvector::MAX_INDEXED_DIMENSIONS),
            "the index cap"
        ),
        i64::try_from(pgvector::MAX_INDEXED_DIMENSIONS).unwrap_or(i64::MAX)
    );
    // A count above `i64::MAX` cannot be bound at all. Reachable only on a 64-bit host, where `usize` and
    // `i64` have the same width, so this asserts the refusal the code path is written for.
    if std::mem::size_of::<usize>() == 8 {
        match dimension_count(usize::MAX) {
            Err(EmbeddingError::Vector(pgvector::PgVectorError::DimensionsTooLarge {
                dimensions,
                ..
            })) => assert_eq!(dimensions, usize::MAX),
            other => panic!("a count above i64::MAX must be refused, got {other:?}"),
        }
    }
}

#[test]
fn a_digest_that_is_not_lowercase_hex_is_refused() {
    // A real digest, and the shapes that are not one. `is_sha256_hex` is the predicate the write path uses
    // before its SQL, and the schema repeats it, so both must agree on all of these.
    assert!(is_sha256_hex(&digest(0x00)));
    assert!(is_sha256_hex(&"f".repeat(64)));
    for bad in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        format!("{}z", "a".repeat(63)),
        "a".repeat(64).replace('a', "-"),
        // A digest-shaped value with a newline is the case a `len() == 64` check alone would miss if the
        // newline replaced a character rather than extending the string.
        format!("{}\n", "a".repeat(63)),
    ] {
        assert!(
            !is_sha256_hex(&bad),
            "`{}` must not be accepted as a SHA-256 digest",
            bad.escape_debug()
        );
    }
    // And a non-finite vector is refused by the codec before the hash is even consulted, which is what makes
    // the hash check a second guard rather than the only one.
    let mut nan = uniform(4, 0.5);
    nan[1] = f32::NAN;
    assert!(matches!(
        pgvector::encode(&nan),
        Err(pgvector::PgVectorError::NonFiniteComponent)
    ));
}

#[test]
fn the_result_bound_is_small_because_the_search_is_a_candidate_source() {
    // Asserted as a comparison rather than as a literal, so changing the value is a decision rather than an
    // edit that passes. The search feeds `jarvis_core::retrieval`'s ranking (`ADR-0046`) instead of ranking
    // itself, and the documented `hnsw.ef_search` default of 40 means a selective filter cannot reliably
    // return more than a few rows anyway.
    let bound = MAX_SIMILARITY_RESULTS;
    let index_cap = pgvector::MAX_INDEXED_DIMENSIONS;
    assert!(
        bound < index_cap,
        "the result bound ({bound}) should stay well below the index cap ({index_cap})"
    );
    assert!(bound >= 10, "a bound below ten would be unusable");
}
