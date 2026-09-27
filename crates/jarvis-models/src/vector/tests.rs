//! Tests for embedding vectors and their metadata.
//!
//! # Why the metadata tests are the important ones
//!
//! `docs/architecture/storage.md`'s rule is "Never compare vectors with incompatible metadata", and a test
//! that only asserted compatible vectors compare happily would pass against an implementation that ignored
//! the metadata entirely. So every incompatible field is asserted to *refuse*, one field at a time, and the
//! compatible case is asserted alongside it as the positive control — without which a comparison that
//! refused everything would pass every refusal assertion.

use super::*;
use crate::identity::{ModelId, ProviderId};
#[allow(unused_imports)]
use crate::testing::{ResultMustErrExt as _, ResultMustExt as _};

fn provider() -> ProviderId {
    ProviderId::new("openai-compatible").must("valid provider")
}

fn model() -> ModelId {
    ModelId::new("text-embedding-3-small").must("valid model")
}

/// Metadata for a 4-dimension vector, with every field overridable at the call site.
fn metadata() -> EmbeddingMetadata {
    EmbeddingMetadata {
        provider: provider(),
        model: model(),
        version: None,
        dimensions: EmbeddingDimensions::new(4).must("valid dimensions"),
        normalization: Normalization::Normalized,
        input_hash: "0".repeat(64),
        chunker_version: None,
    }
}

/// A vector of the given components, validated.
fn vector(values: &[f32]) -> EmbeddingVector {
    EmbeddingVector::new(values.to_vec()).must("valid vector")
}

/// An embedding pairing a four-component vector with metadata.
fn embedding(values: &[f32]) -> Embedding {
    Embedding::new(vector(values), metadata()).must("consistent pair")
}

// ------------------------------------------------------------------------------------------------
// The metadata guard
// ------------------------------------------------------------------------------------------------

/// **Every metadata field that must agree actually refuses a mismatch, one field at a time.**
///
/// The rule from `docs/architecture/storage.md`. Each field is mutated in isolation so only the field under
/// test can be the cause, and the compatible case is asserted first as the positive control — a comparison
/// that refused everything would otherwise pass every assertion below.
///
/// `input_hash` and `chunker_version` are deliberately absent from the refusal list: two vectors of different
/// text are exactly what a similarity search compares, so requiring them to match would make the only
/// meaningful comparison impossible.
#[test]
fn incompatible_metadata_is_refused_field_by_field() {
    let left = embedding(&[1.0, 0.0, 0.0, 0.0]);
    assert_eq!(
        left.metadata().ensure_comparable_with(left.metadata()),
        Ok(()),
        "identical metadata must be comparable, or the guard refuses everything"
    );

    let other_provider = EmbeddingMetadata {
        provider: ProviderId::new("local-embeddings").must("valid"),
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&other_provider),
        Err(EmbeddingError::IncompatibleMetadata { field: "provider" })
    );

    let other_model = EmbeddingMetadata {
        model: ModelId::new("text-embedding-3-large").must("valid"),
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&other_model),
        Err(EmbeddingError::IncompatibleMetadata { field: "model" })
    );

    let versioned = EmbeddingMetadata {
        version: Some(EmbeddingVersion::new("2024-02-01").must("valid")),
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&versioned),
        Err(EmbeddingError::IncompatibleMetadata { field: "version" })
    );

    let narrower = EmbeddingMetadata {
        dimensions: EmbeddingDimensions::new(2).must("valid"),
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&narrower),
        Err(EmbeddingError::IncompatibleMetadata {
            field: "dimensions"
        })
    );

    let unnormalized = EmbeddingMetadata {
        normalization: Normalization::Unnormalized,
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&unnormalized),
        Err(EmbeddingError::IncompatibleMetadata {
            field: "normalization"
        })
    );

    // The two fields that are allowed to differ, because comparing different text is the point.
    let different_input = EmbeddingMetadata {
        input_hash: "f".repeat(64),
        chunker_version: Some("chunker-2".to_owned()),
        ..metadata()
    };
    assert_eq!(
        metadata().ensure_comparable_with(&different_input),
        Ok(()),
        "a comparison between different inputs is what a similarity search does"
    );
}

/// **`Normalization` defaults to `Unknown`, and only `Normalized` permits the dot-product shortcut.**
///
/// The permissive assumption is the one that ranks wrongly rather than failing. A default of `Normalized`
/// would silently apply the researched provider's property to every provider, and a dot product against
/// unnormalized vectors produces a plausible number that is not a similarity.
#[test]
fn only_a_declared_normalization_permits_the_shortcut() {
    assert_eq!(
        Normalization::default(),
        Normalization::Unknown,
        "an unestablished property must not default to the permissive reading"
    );
    assert!(Normalization::Normalized.allows_dot_product());
    assert!(!Normalization::Unnormalized.allows_dot_product());
    assert!(
        !Normalization::Unknown.allows_dot_product(),
        "an unknown property must not enable the shortcut"
    );

    // The names are stable, because a stored normalization is read back by name.
    for value in [
        Normalization::Normalized,
        Normalization::Unnormalized,
        Normalization::Unknown,
    ] {
        assert!(!value.as_str().is_empty());
    }
}

// ------------------------------------------------------------------------------------------------
// The constructor's checks
// ------------------------------------------------------------------------------------------------

/// **A vector whose length contradicts its metadata is refused, which is the check that makes the guard real.**
///
/// Without it, a provider could return a different shape than requested and the value would be stored with
/// metadata describing something else — and the metadata guard would then compare two vectors it believed were
/// the same shape. The assertion is on the *pair*, not on the vector alone, because a vector has no dimension
/// of its own to be wrong about.
#[test]
fn a_vector_length_that_contradicts_its_metadata_is_refused() {
    let mismatch = Embedding::new(vector(&[1.0, 0.0, 0.0]), metadata());
    assert_eq!(mismatch, Err(EmbeddingError::DimensionMismatch));

    // The control: the length the metadata declares is accepted, so the refusal is about the disagreement.
    assert!(Embedding::new(vector(&[1.0, 0.0, 0.0, 0.0]), metadata()).is_ok());
}

/// **A non-finite component is refused, because it makes every distance undefined rather than large.**
///
/// A `NaN` in a stored vector poisons every later query that touches it, and the failure reads as a corrupted
/// index rather than as something a provider returned.
#[test]
fn a_non_finite_component_is_refused() {
    assert_eq!(
        EmbeddingVector::new(vec![1.0, f32::NAN, 0.0]),
        Err(EmbeddingError::NonFiniteComponent)
    );
    assert_eq!(
        EmbeddingVector::new(vec![f32::INFINITY]),
        Err(EmbeddingError::NonFiniteComponent)
    );
    assert_eq!(
        EmbeddingVector::new(vec![f32::NEG_INFINITY]),
        Err(EmbeddingError::NonFiniteComponent)
    );
    assert_eq!(
        EmbeddingVector::new(Vec::new()),
        Err(EmbeddingError::VectorLength),
        "an empty vector is not a vector"
    );
    assert!(EmbeddingVector::new(vec![0.0, -1.5]).is_ok());
}

/// **An unusable metadata field is refused by name, and the messages never carry the value.**
///
/// A hash reaches a database column and a log, so a value with whitespace or a control character must not
/// arrive there — and the reason names the *field* rather than echoing the value, because the value could be
/// user content on the chunker path.
#[test]
fn unusable_metadata_is_refused_without_echoing_it() {
    let empty_hash = EmbeddingMetadata {
        input_hash: String::new(),
        ..metadata()
    };
    assert_eq!(
        empty_hash.validate(),
        Err(EmbeddingError::MetadataUnusable {
            field: "input_hash"
        })
    );

    let non_hex = EmbeddingMetadata {
        input_hash: "not a hex digest".to_owned(),
        ..metadata()
    };
    assert_eq!(
        non_hex.validate(),
        Err(EmbeddingError::MetadataUnusable {
            field: "input_hash"
        })
    );

    let control = EmbeddingMetadata {
        input_hash: "00\u{1f}00".to_owned(),
        ..metadata()
    };
    assert_eq!(
        control.validate(),
        Err(EmbeddingError::MetadataUnusable {
            field: "input_hash"
        })
    );

    let bad_chunker = EmbeddingMetadata {
        chunker_version: Some(String::new()),
        ..metadata()
    };
    assert_eq!(
        bad_chunker.validate(),
        Err(EmbeddingError::MetadataUnusable {
            field: "chunker_version"
        })
    );

    // The control: a well-formed set is accepted, including a chunker version that is present.
    let good = EmbeddingMetadata {
        chunker_version: Some("chunker-1".to_owned()),
        ..metadata()
    };
    assert_eq!(good.validate(), Ok(()));

    // And the error text names the field rather than the value.
    let text = format!(
        "{}",
        EmbeddingError::MetadataUnusable {
            field: "input_hash"
        }
    );
    assert!(text.contains("input_hash"));
    assert!(
        !text.contains("not a hex"),
        "an error must not carry the value it rejected"
    );
}

/// **A dimension outside the accepted range is refused, and the bound is far above every real model.**
///
/// The bound exists because a provider response is untrusted input. It is asserted to be above the largest
/// dimension a researched model returns, so the refusal cannot fire on a legitimate vector.
#[test]
fn the_dimension_bound_is_above_every_real_model() {
    assert_eq!(
        EmbeddingDimensions::new(0),
        Err(EmbeddingError::DimensionsUnusable)
    );
    assert_eq!(
        EmbeddingDimensions::new(EmbeddingDimensions::MAX + 1),
        Err(EmbeddingError::DimensionsUnusable)
    );
    assert!(EmbeddingDimensions::new(1).is_ok());
    assert!(EmbeddingDimensions::new(EmbeddingDimensions::MAX).is_ok());

    // `text-embedding-3-large` returns 3072, the largest of the researched models, and a shortened vector can
    // be as small as 256. Both must be inside the range.
    for dimension in [256, 1024, 1536, 3072] {
        assert!(
            EmbeddingDimensions::new(dimension).is_ok(),
            "{dimension} must be an accepted dimension"
        );
        assert!(
            dimension <= EmbeddingDimensions::MAX,
            "{dimension} must be below the bound, or the bound refuses a real model"
        );
    }
}

/// **A version is bounded, non-empty, and control-free.**
///
/// The version is opaque so nothing here can mis-order it, but it reaches a column and a log, so its shape is
/// checked. The bound matches the identifier vocabulary's, because both values reach the same places.
#[test]
fn a_version_is_bounded_and_control_free() {
    assert_eq!(
        EmbeddingVersion::new(""),
        Err(EmbeddingError::VersionUnusable)
    );
    assert_eq!(
        EmbeddingVersion::new("a".repeat(EmbeddingVersion::MAX_BYTES + 1)),
        Err(EmbeddingError::VersionUnusable)
    );
    assert_eq!(
        EmbeddingVersion::new("v1\u{0}"),
        Err(EmbeddingError::VersionUnusable)
    );
    assert_eq!(
        EmbeddingVersion::new("a".repeat(EmbeddingVersion::MAX_BYTES))
            .must("at the bound")
            .as_str()
            .len(),
        EmbeddingVersion::MAX_BYTES
    );
}

// ------------------------------------------------------------------------------------------------
// Similarity
// ------------------------------------------------------------------------------------------------

/// **Cosine similarity is the dot product only when the metadata declares normalization.**
///
/// The property the `Normalization` field exists for. The fixture is a pair of *unnormalized* vectors whose
/// dot product and cosine differ, so an implementation that always used the dot product produces a visibly
/// wrong number rather than a plausible one — and an implementation that always divided produces the same
/// answer twice, which the comparison against the known cosine catches.
#[test]
fn the_dot_product_shortcut_requires_a_declared_normalization() {
    // `[3, 4]` has magnitude 5, so it is deliberately not unit length.
    let long_a = embedding(&[3.0, 4.0, 0.0, 0.0]);
    let long_b = embedding(&[6.0, 8.0, 0.0, 0.0]);

    // Declared normalized: the caller's metadata says the magnitudes are 1, so the dot product is the answer.
    let dot = cosine_similarity(&long_a, &long_b).must("comparable");
    assert!(
        (dot - 50.0).abs() < 1e-6,
        "a declared-normalized pair is compared by dot product, got {dot}"
    );

    // Declared unnormalized: the magnitudes are divided out, and 50/25 is 2.0.
    let unnormalized = |values: &[f32]| {
        Embedding::new(
            vector(values),
            EmbeddingMetadata {
                normalization: Normalization::Unnormalized,
                ..metadata()
            },
        )
        .must("consistent pair")
    };
    let cosine = cosine_similarity(
        &unnormalized(&[3.0, 4.0, 0.0, 0.0]),
        &unnormalized(&[6.0, 8.0, 0.0, 0.0]),
    )
    .must("comparable");
    assert!(
        (cosine - 1.0).abs() < 1e-6,
        "parallel vectors are maximally similar whatever their magnitude, got {cosine}"
    );

    // Orthogonal vectors are uncorrelated under either declaration, which is the control that the arithmetic
    // is a similarity rather than a scaled dot product.
    let orthogonal = cosine_similarity(
        &unnormalized(&[1.0, 0.0, 0.0, 0.0]),
        &unnormalized(&[0.0, 1.0, 0.0, 0.0]),
    )
    .must("comparable");
    assert!(
        orthogonal.abs() < 1e-6,
        "orthogonal vectors, got {orthogonal}"
    );
}

/// **A comparison across incompatible metadata refuses rather than returning a number.**
///
/// The whole point of the guard, asserted through the operation a caller actually calls rather than through
/// `ensure_comparable_with` alone — a `cosine_similarity` that forgot to consult the metadata would pass the
/// field-by-field test and fail this one.
#[test]
fn a_comparison_across_models_refuses() {
    let small = embedding(&[1.0, 0.0, 0.0, 0.0]);
    let large = Embedding::new(
        vector(&[1.0, 0.0, 0.0, 0.0]),
        EmbeddingMetadata {
            model: ModelId::new("text-embedding-3-large").must("valid"),
            ..metadata()
        },
    )
    .must("consistent pair");

    assert_eq!(
        cosine_similarity(&small, &large),
        Err(EmbeddingError::IncompatibleMetadata { field: "model" })
    );
}

/// **A zero vector refuses a comparison rather than reporting zero similarity.**
///
/// A vector with no magnitude has no direction, so its similarity to anything is undefined. Returning `0.0`
/// would report "unrelated" for a value that is not a direction at all, and a caller would rank it as merely
/// dissimilar. This can only be reached for an unnormalized declaration, because a normalized vector of all
/// zeros contradicts the declaration — which is itself worth noting: the guard is reachable, so it is tested.
#[test]
fn a_zero_vector_refuses_a_comparison() {
    let zero = Embedding::new(
        vector(&[0.0, 0.0, 0.0, 0.0]),
        EmbeddingMetadata {
            normalization: Normalization::Unnormalized,
            ..metadata()
        },
    )
    .must("consistent pair");
    let other = Embedding::new(
        vector(&[1.0, 0.0, 0.0, 0.0]),
        EmbeddingMetadata {
            normalization: Normalization::Unnormalized,
            ..metadata()
        },
    )
    .must("consistent pair");
    assert_eq!(
        cosine_similarity(&zero, &other),
        Err(EmbeddingError::VectorLength),
        "a zero vector has no direction, so it has no similarity"
    );
}

/// **A serialized embedding round-trips, so a stored vector is the value that comes back.**
///
/// The metadata is the reason: a round trip that dropped it would return a vector whose comparison rules are
/// gone, and the two would be indistinguishable in the JSON.
#[test]
fn an_embedding_round_trips_through_json() {
    let original = embedding(&[0.5, -0.25, 0.125, 0.0]);
    let json = serde_json::to_string(&original).must("serializes");
    let decoded: Embedding = serde_json::from_str(&json).must("decodes");
    assert_eq!(decoded, original);
    assert_eq!(decoded.metadata(), original.metadata());

    // And the round-tripped value still refuses an incompatible comparison, so the guard survived.
    let other = Embedding::new(
        vector(&[0.5, -0.25, 0.125, 0.0]),
        EmbeddingMetadata {
            normalization: Normalization::Unnormalized,
            ..metadata()
        },
    )
    .must("consistent pair");
    assert!(cosine_similarity(&decoded, &other).is_err());
}
