//! The pgvector text form: encoding embedding values into PostgreSQL and reading them back.
//!
//! `P4-009` requires PostgreSQL plus pgvector parity for the memory behaviour, and
//! `docs/research/integrations/postgres-pgvector.md` records the external contract. The decisive finding
//! there shapes this module:
//!
//! > **SQLx 0.9.0 has no `vector` type mapping.** The Postgres driver's type table lists `bool`, the
//! > integers, `f32`/`f64`, strings, `BYTEA`, `UUID`, `JSON`/`JSONB`, `TIMESTAMPTZ`, and others — and no
//! > vector type of any kind. So the value crosses the boundary as a **text literal** and this module is
//! > the code that has to be right.
//!
//! pgvector's documented text form is `'[1,2,3]'`, and `0.4.0` "Changed text representation for vector
//! elements to match `real`" — so the format is Postgres's own `real` output, not an approximation of it.
//!
//! # Why this takes `&[f32]` and not the domain's embedding type
//!
//! The embedding types live in `jarvis-models` (`EmbeddingVector`, `EmbeddingMetadata`), and
//! `docs/architecture/repository-layout.md`'s dependency graph allows adapters to depend on `jarvis-core`
//! and `jarvis-protocol` and **not on each other**. `jarvis-storage` and `jarvis-models` are both adapters,
//! so a dependency between them is forbidden — the same rule `ADR-0047` applied when it kept the embedding
//! port's types inside the provider boundary.
//!
//! So the boundary here is plain data: a slice of `f32` going out and a `Vec<f32>` coming back. The
//! composition root, which may see both crates, maps between them. That is the correct narrow waist rather
//! than a compromise, and it makes this module testable with no model crate, no provider, and no database.
//!
//! # What is refused, and why the bound is 2,000 rather than 16,000
//!
//! pgvector stores up to 16,000 dimensions but **indexes a `vector` column only up to 2,000** (its README's
//! `HNSW` and `IVFFlat` sections both state the cap). A column that accepted a vector the index cannot hold
//! would make "we use pgvector" true while retrieval silently fell back to an exact scan over every row — a
//! performance claim with nothing behind it. So a dimension above the indexed cap is refused **by name**,
//! with the documented alternatives in the message, rather than stored and quietly unindexable.
//!
//! The domain's own `EmbeddingDimensions::MAX` is 16,384, eight times this cap. That gap is a **recorded
//! fact and a defect in neither crate**: the domain bounds what a provider may return, and this bounds what
//! pgvector can search. The refusal below is where the two meet.

use std::fmt;

/// The most dimensions pgvector can index in a `vector` column.
///
/// From the pgvector README's `HNSW` and `IVFFlat` sections: "Supported types are: `vector` - up to 2,000
/// dimensions, `halfvec` - up to 4,000 dimensions, `bit` - up to 64,000 dimensions". The *storage* limit is
/// 16,000; this is the **index** limit, which is the one that decides whether a nearest-neighbour query is
/// an index scan or a full scan.
pub const MAX_INDEXED_DIMENSIONS: usize = 2_000;

/// Why embedding values could not be written to, or read from, a pgvector column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PgVectorError {
    /// The vector has more dimensions than pgvector can index.
    DimensionsTooLarge {
        /// The vector's dimension count.
        dimensions: usize,
        /// [`MAX_INDEXED_DIMENSIONS`].
        maximum: usize,
    },
    /// The value is not a pgvector vector literal.
    ///
    /// Carries nothing from the text: a malformed column could contain anything, and a diagnostic that
    /// echoed it would put an unknown value into a log line.
    Malformed,
    /// The value holds a component that is not a finite number.
    ///
    /// Separate from [`Self::Malformed`] because a value that parses but is `NaN` or infinite is a
    /// *different* problem from one that does not parse, and the two are fixed differently.
    NonFiniteComponent,
    /// The vector holds no components.
    ///
    /// An empty vector is not a vector: every distance involving it is undefined, and accepting one would
    /// let a row that means "no embedding" be compared as though it had one.
    Empty,
}

impl fmt::Display for PgVectorError {
    /// Renders the failure for an operator, naming the alternatives where the external contract offers them.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionsTooLarge {
                dimensions,
                maximum,
            } => write!(
                formatter,
                "pgvector can index a `vector` column only up to {maximum} dimensions, and this embedding \
                 has {dimensions}; the extension's documented alternatives above that are `halfvec` (up to \
                 4,000), half-precision indexing, binary quantization with re-ranking, subvector indexing, \
                 or dimensionality reduction"
            ),
            Self::Malformed => formatter.write_str(
                "the stored value is not a pgvector vector literal of the form `[1,2,3]`",
            ),
            Self::NonFiniteComponent => {
                formatter.write_str("the vector holds a component that is not a finite number")
            }
            Self::Empty => formatter.write_str("the vector holds no components"),
        }
    }
}

impl std::error::Error for PgVectorError {}

/// Encodes embedding values into pgvector's text form.
///
/// # Errors
///
/// Returns [`PgVectorError::Empty`] for no components, [`PgVectorError::NonFiniteComponent`] for a `NaN` or
/// infinite value, and [`PgVectorError::DimensionsTooLarge`] above [`MAX_INDEXED_DIMENSIONS`]. See the module
/// doc: the dimension is refused here rather than at the column, because a stored-but-unindexable vector is
/// a retrieval that silently stops being a search.
pub fn encode(values: &[f32]) -> Result<String, PgVectorError> {
    if values.is_empty() {
        return Err(PgVectorError::Empty);
    }
    if values.len() > MAX_INDEXED_DIMENSIONS {
        return Err(PgVectorError::DimensionsTooLarge {
            dimensions: values.len(),
            maximum: MAX_INDEXED_DIMENSIONS,
        });
    }
    // Checked before formatting rather than left to the decode, because a `NaN` **formats successfully** as
    // the text `NaN` — so a guard that only guarded the read would let this platform write a row it cannot
    // read, and the write would look like it worked.
    if values.iter().any(|value| !value.is_finite()) {
        return Err(PgVectorError::NonFiniteComponent);
    }
    // pgvector's elements are Postgres `real`, and `f32`'s `Display` writes the shortest decimal string that
    // round-trips to the same `f32` — which is the property the round trip below asserts. A fixed number of
    // decimals would be shorter to read and lossy, and losing a component's low bits is exactly the defect
    // that reads as a small ranking difference rather than as an error.
    let mut rendered = String::with_capacity(values.len() * 12 + 2);
    rendered.push('[');
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        // `write!` into the buffer rather than `push_str(&format!(..))`, and the `Result` is discarded
        // because writing to a `String` cannot fail.
        let _ = fmt::Write::write_fmt(&mut rendered, format_args!("{value}"));
    }
    rendered.push(']');
    Ok(rendered)
}

/// Decodes pgvector's text form back into embedding values.
///
/// # Errors
///
/// Returns [`PgVectorError::Malformed`] for anything that is not a bracketed comma-separated list,
/// [`PgVectorError::NonFiniteComponent`] for a component that parses but is not finite, and
/// [`PgVectorError::Empty`] for `[]`.
pub fn decode(text: &str) -> Result<Vec<f32>, PgVectorError> {
    let body = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or(PgVectorError::Malformed)?;
    // An empty body is `[]`, which is not a vector and gets its own reason rather than "malformed": the text
    // is well formed and it is the *content* that is unacceptable — the same distinction the gateway draws
    // between a `400` and a `422`.
    if body.is_empty() {
        return Err(PgVectorError::Empty);
    }
    let mut values = Vec::new();
    for component in body.split(',') {
        let value: f32 = component
            .trim()
            .parse()
            .map_err(|_| PgVectorError::Malformed)?;
        // Checked explicitly, because `"NaN".parse::<f32>()` **succeeds**: a decoder without this admits a
        // `NaN` into a comparison, where it makes every distance `NaN` and the row quietly ranks last rather
        // than reporting anything.
        if !value.is_finite() {
            return Err(PgVectorError::NonFiniteComponent);
        }
        values.push(value);
    }
    Ok(values)
}

/// Returns whether a dimension count can be indexed by pgvector.
///
/// Exposed so a caller can ask **before** embedding, rather than discovering it from an error: an embedding
/// service can decline to embed a value it could not store, which is cheaper and does not spend a provider
/// call on a result that would be discarded.
#[must_use]
pub const fn is_indexable(dimensions: usize) -> bool {
    dimensions > 0 && dimensions <= MAX_INDEXED_DIMENSIONS
}

/// The distance operator a query must order by.
///
/// # Why names and not a formatted string
///
/// The README's Troubleshooting section names the shape that defeats the index: "the `ORDER BY` must be the
/// result of a distance operator (not an expression) in ascending order", with
/// `ORDER BY 1 - (embedding <=> '[3,1,2]') DESC` given as the example that uses **no** index. The natural
/// way to express cosine *similarity* is exactly that expression, so a caller writing a query by hand would
/// reach for it. Naming the operator and the class here means the correct shape is the one this module
/// offers, and `ADR-0051` records why the query is shaped by the index rather than the reverse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DistanceMetric {
    /// `<=>`, cosine distance. The default for this platform — see the research record.
    Cosine,
    /// `<->`, Euclidean distance.
    Euclidean,
    /// `<#>`, **negative** inner product.
    NegativeInnerProduct,
}

impl DistanceMetric {
    /// Returns the SQL operator, which is what a query must place in its `ORDER BY`.
    #[must_use]
    pub const fn operator(self) -> &'static str {
        match self {
            Self::Cosine => "<=>",
            Self::Euclidean => "<->",
            Self::NegativeInnerProduct => "<#>",
        }
    }

    /// Returns the operator class an index for this metric must use.
    ///
    /// The README: "Add an index for each distance function you want to use." So the index and the query are
    /// a pair, and a query using a metric with no index is an exact scan that looks like a working search.
    #[must_use]
    pub const fn operator_class(self) -> &'static str {
        match self {
            Self::Cosine => "vector_cosine_ops",
            Self::Euclidean => "vector_l2_ops",
            Self::NegativeInnerProduct => "vector_ip_ops",
        }
    }

    /// Returns whether an ascending value means "closer" for this metric.
    ///
    /// `true` for every operator pgvector offers, and that is the point rather than a tautology: the README
    /// says `<#>` is negated *because* "Postgres only supports `ASC` order index scans on operators". A named
    /// predicate gives a caller converting a stored value into a similarity one place to ask, so the negation
    /// is asserted rather than remembered.
    #[must_use]
    pub const fn ascending_is_closer(self) -> bool {
        true
    }

    /// Converts a stored distance into a similarity, which is the direction the ranking needs.
    ///
    /// # Why this is a method and not arithmetic at the call site
    ///
    /// Two of the three conversions are negations and one is `1 - d`, and getting it wrong produces a
    /// plausible ranking that is *reversed* or *shifted* rather than an error. `jarvis_core::retrieval`
    /// scores cosine **similarity** in `[-1, 1]`, so the value that leaves this module has to be in those
    /// units or the semantic weight means something different from what `ADR-0048` recorded.
    #[must_use]
    pub fn similarity(self, distance: f32) -> f32 {
        match self {
            // Cosine distance is `1 - similarity`, so similarity is `1 - distance`.
            Self::Cosine => 1.0 - distance,
            // Both remaining metrics store a value that is **already negated or directional**, so the
            // conversion is a negation rather than a subtraction from one.
            //
            // - `<#>` is documented as the negative inner product, so a stored value is a similarity's
            //   negation.
            // - Euclidean distance is non-negative and unbounded above, so there is no similarity in the
            //   ranking's units at all. Every reciprocal-shaped alternative (`1 / (1 + d)`) invents a
            //   constant, so this negates instead: that orders identically to the reciprocal while staying
            //   linear, and it is documented as an **ordering** rather than a similarity so nothing treats
            //   it as one.
            Self::Euclidean | Self::NegativeInnerProduct => -distance,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The round trip, which is the central assumption.** `decode(encode(v))` must return the values that
    /// went in, compared by bit pattern.
    ///
    /// Values are chosen to break a naive formatter rather than to be readable: `1.0 / 3.0` is not exactly
    /// representable, `f32::MIN_POSITIVE` is the smallest normal number, and `-0.0` is the case where a value
    /// comparison and a bit comparison disagree. A formatter that printed a fixed number of decimals would
    /// pass with `1.5` and fail here.
    #[test]
    fn a_vector_round_trips_exactly() {
        let cases: &[&[f32]] = &[
            &[1.5, -2.25, 0.0],
            &[0.0, -0.0],
            &[1.0 / 3.0, f32::MIN_POSITIVE, f32::MAX],
            &[f32::MIN, -1.0e-30, 3.402_823_5e38],
            &[1.0],
            &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
        ];
        for case in cases {
            let encoded = encode(case).unwrap_or_else(|error| panic!("encode {case:?}: {error}"));
            let decoded =
                decode(&encoded).unwrap_or_else(|error| panic!("decode {encoded}: {error}"));
            // The bit patterns, not just the values: `-0.0 == 0.0` is true, so a value assertion alone
            // cannot see a sign lost by the text form, and a sign lost on a zero is invisible until a
            // distance produces a `-0.0` in a stored explanation.
            let expected: Vec<u32> = case.iter().map(|value| value.to_bits()).collect();
            let actual: Vec<u32> = decoded.iter().map(|value| value.to_bits()).collect();
            assert_eq!(
                actual, expected,
                "the round trip changed a bit pattern: {case:?} became {encoded}"
            );
        }
    }

    /// The encoder writes pgvector's documented form.
    ///
    /// The README's examples are `'[1,2,3]'` and `'[3,1,2]'`, so the shape is bracketed, comma-separated,
    /// and carries no spaces. Asserted literally because a round-trip test alone cannot tell a correct form
    /// from a self-consistent wrong one, and a wrong form is rejected by the server at insert time.
    #[test]
    fn the_encoder_writes_the_documented_form() {
        assert_eq!(
            encode(&[1.0, 2.0, 3.0]).unwrap_or_default(),
            "[1,2,3]",
            "pgvector's README inserts vectors as `'[1,2,3]'`"
        );
        assert_eq!(encode(&[3.0, 1.0, 2.0]).unwrap_or_default(), "[3,1,2]");
        // A negative and a fractional value, because `-0.5` is where a locale-dependent or integer-only
        // formatter diverges, and the README notes 0.4.0 changed the representation to match `real`.
        assert_eq!(encode(&[-0.5, 1.25]).unwrap_or_default(), "[-0.5,1.25]");
    }

    /// **A vector pgvector cannot index is refused by name, not stored.**
    ///
    /// This is the falsification test for the dimension guard: with the check removed, a 2,001-dimension
    /// vector encodes successfully and the caller has no way to know the nearest-neighbour query will be a
    /// full scan.
    #[test]
    fn a_vector_above_the_indexed_cap_is_refused() {
        let too_many = vec![0.0_f32; MAX_INDEXED_DIMENSIONS + 1];
        match encode(&too_many) {
            Err(PgVectorError::DimensionsTooLarge {
                dimensions,
                maximum,
            }) => {
                assert_eq!(dimensions, MAX_INDEXED_DIMENSIONS + 1);
                assert_eq!(maximum, MAX_INDEXED_DIMENSIONS);
            }
            other => panic!("a dimension above the indexed cap must be refused, got {other:?}"),
        }

        // The exact cap must be **accepted**, or the guard is a wall that refuses the largest supported value
        // and the test above would pass with the comparison inverted.
        let at_cap = vec![0.0_f32; MAX_INDEXED_DIMENSIONS];
        let encoded =
            encode(&at_cap).unwrap_or_else(|error| panic!("the cap must be accepted: {error}"));
        assert_eq!(
            decode(&encoded).unwrap_or_default().len(),
            MAX_INDEXED_DIMENSIONS
        );

        // And one **fewer** must be accepted, so an off-by-one that rejected the cap is also caught.
        assert!(encode(&vec![0.0_f32; MAX_INDEXED_DIMENSIONS - 1]).is_ok());
    }

    /// **The finite-only guard is on both sides, and the encode side is not redundant.**
    ///
    /// `NaN` formats successfully as the text `NaN`, so without the encode-side check this platform would
    /// write a row it cannot read back — and the write would succeed, so the defect would be invisible until
    /// a read. This is the direction the falsification matters in.
    #[test]
    fn a_non_finite_component_cannot_be_encoded_or_decoded() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let refused = encode(&[1.0, bad]);
            assert!(
                matches!(refused, Err(PgVectorError::NonFiniteComponent)),
                "{bad} must not be encodable, got {refused:?}"
            );
        }
        // The decode side, where a value written by something else would arrive. `inf` and `-inf` are what
        // `f32::Display` writes, so these texts are the round trip of the values above rather than arbitrary
        // spellings.
        for text in ["[NaN,1]", "[1,inf]", "[inf,1]", "[-inf,1]", "[1,-NaN]"] {
            let refused = decode(text);
            assert!(
                matches!(refused, Err(PgVectorError::NonFiniteComponent)),
                "{text} must report a non-finite component, got {refused:?}"
            );
        }
    }

    /// Every malformed shape is refused, and each for the reason that names it.
    ///
    /// A decoder that accepted any of these would put a wrong vector into a comparison, where it would
    /// produce a plausible similarity rather than an error — the failure mode this repository treats as the
    /// worst kind. The cases include the ones that *look* almost right: an unbalanced bracket, a trailing
    /// comma, a space-separated list, and pgvector's own `{...}` sparse form.
    #[test]
    fn a_malformed_literal_is_refused() {
        for (label, text) in [
            ("no brackets", "1,2,3"),
            ("unbalanced open", "[1,2,3"),
            ("unbalanced close", "1,2,3]"),
            ("trailing comma", "[1,2,]"),
            ("leading comma", "[,1,2]"),
            ("only a comma", "[,]"),
            ("space separated", "[1 2 3]"),
            ("not a number", "[a,b,c]"),
            ("sparsevec form", "{1:1,3:2}/5"),
            ("nested", "[[1,2]]"),
            ("empty string", ""),
            ("json null", "null"),
            ("only brackets", "[]"),
        ] {
            let refused = decode(text);
            // `[]` is the one well-formed-but-empty case, and it is asserted separately below; here it must
            // at least be refused, so `unwrap`-style leniency cannot admit it.
            assert!(
                matches!(
                    refused,
                    Err(PgVectorError::Malformed | PgVectorError::Empty)
                ),
                "{label} must be refused, got {refused:?}"
            );
        }
        // The specific shapes, so the list above cannot pass by every case collapsing into one reason.
        assert!(matches!(decode("[1,2,]"), Err(PgVectorError::Malformed)));
        assert!(matches!(decode("[1 2 3]"), Err(PgVectorError::Malformed)));
    }

    /// An empty vector is its own reason at both ends, and it is not "malformed": `[]` is well-formed text
    /// whose content is unacceptable, and `encode(&[])` is a caller's mistake rather than a parse failure.
    #[test]
    fn an_empty_vector_is_refused_as_empty_rather_than_malformed() {
        assert!(matches!(encode(&[]), Err(PgVectorError::Empty)));
        assert!(matches!(decode("[]"), Err(PgVectorError::Empty)));
    }

    /// A full-width vector is encoded and decoded without truncation, because a silent truncation would
    /// produce a shorter vector that is still a valid one — so nothing downstream could tell.
    ///
    /// The values cycle through a small exact set rather than being computed from the index: an index cast to
    /// `f32` is a precision-loss lint, and a fixture that trips one would either need an `allow` or become an
    /// excuse to weaken the lint workspace-wide. Exact values also make the comparison meaningful, since a
    /// computed value would be compared against its own re-derivation.
    #[test]
    fn a_full_width_vector_survives() {
        let pattern = [0.5_f32, -1.25, 0.0, 7.0, -0.062_5];
        let values: Vec<f32> = pattern
            .iter()
            .copied()
            .cycle()
            .take(MAX_INDEXED_DIMENSIONS)
            .collect();
        let encoded = encode(&values).unwrap_or_else(|error| panic!("{error}"));
        let decoded = decode(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded.len(), values.len());
        assert_eq!(decoded, values);
        // The component count in the text, so a formatter that dropped the last element and closed the
        // bracket early is caught by something other than the round trip that shares its loop.
        assert_eq!(
            encoded.matches(',').count(),
            MAX_INDEXED_DIMENSIONS - 1,
            "the encoding must hold one separator fewer than there are components"
        );
    }

    /// **The index and the query must agree on the metric, and the negation is asserted rather than
    /// remembered.**
    ///
    /// The README is explicit that `<#>` is negated because Postgres only supports `ASC` index scans, so a
    /// caller that read a stored value as a similarity would rank **backwards** while every type checked.
    #[test]
    fn each_metric_names_its_operator_and_index_class() {
        // Exactly as the README documents them, and paired, because the README says an index is needed per
        // distance function: a query using a metric with no matching index is an exact scan.
        assert_eq!(DistanceMetric::Cosine.operator(), "<=>");
        assert_eq!(DistanceMetric::Cosine.operator_class(), "vector_cosine_ops");
        assert_eq!(DistanceMetric::Euclidean.operator(), "<->");
        assert_eq!(DistanceMetric::Euclidean.operator_class(), "vector_l2_ops");
        assert_eq!(DistanceMetric::NegativeInnerProduct.operator(), "<#>");
        assert_eq!(
            DistanceMetric::NegativeInnerProduct.operator_class(),
            "vector_ip_ops"
        );

        // Every metric orders ascending, which is what makes an `ASC` index scan usable and is why `<#>` is
        // documented as a negative inner product rather than an inner product.
        for metric in [
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::NegativeInnerProduct,
        ] {
            assert!(
                metric.ascending_is_closer(),
                "{metric:?} must order ascending to use its index"
            );
        }
    }

    /// **The similarity conversion, which is where a subtle inversion would hide.**
    ///
    /// `jarvis_core::retrieval` scores cosine similarity in `[-1, 1]`, so a stored distance has to be
    /// converted into the same units. A cosine distance of `0` means identical vectors, so the similarity
    /// must be `1`; a distance of `2` means opposed vectors, so it must be `-1`. Asserting those two
    /// endpoints pins the direction, and an inverted conversion would give `-1` for identical vectors while
    /// still type-checking and still producing a ranking.
    #[test]
    fn a_cosine_distance_converts_to_the_similarity_the_ranking_scores() {
        let cosine = DistanceMetric::Cosine;
        assert!((cosine.similarity(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((cosine.similarity(2.0) - (-1.0)).abs() < f32::EPSILON);
        assert!(cosine.similarity(1.0).abs() < f32::EPSILON);
        // Monotonic decreasing: a larger distance must never give a larger similarity, which is the property
        // an inversion would break. The distances are a fixed list rather than computed from a counter, so
        // the assertion compares against values a reader can check and no index cast is needed.
        let mut previous = f32::INFINITY;
        for distance in [0.0_f32, 0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
            let similarity = cosine.similarity(distance);
            assert!(
                similarity < previous,
                "similarity must decrease as distance grows: d={distance} gave {similarity} after {previous}"
            );
            previous = similarity;
        }

        // The negated metrics: their stored value is already a negation, so a *smaller* value is closer and
        // the conversion negates rather than subtracting from one.
        for negated in [
            DistanceMetric::NegativeInnerProduct,
            DistanceMetric::Euclidean,
        ] {
            assert!(
                negated.similarity(0.5) > negated.similarity(2.0),
                "{negated:?} must keep a smaller distance closer"
            );
        }
    }

    /// The indexability predicate answers for both edges, so a caller that asks before embedding gets the
    /// same answer the encoder would give.
    #[test]
    fn indexability_agrees_with_the_encoder() {
        for dimensions in [1, MAX_INDEXED_DIMENSIONS - 1, MAX_INDEXED_DIMENSIONS] {
            let values = vec![0.0_f32; dimensions];
            assert!(
                encode(&values).is_ok(),
                "a vector of {dimensions} dimensions must encode"
            );
            assert!(is_indexable(dimensions), "and must be indexable");
        }
        // The two edges where the predicate and the encoder must agree on a **refusal**, which is the
        // direction a caller actually relies on: asking before paying for an embedding.
        assert!(!is_indexable(0));
        assert!(matches!(encode(&[]), Err(PgVectorError::Empty)));
        assert!(!is_indexable(MAX_INDEXED_DIMENSIONS + 1));
        assert!(matches!(
            encode(&vec![0.0_f32; MAX_INDEXED_DIMENSIONS + 1]),
            Err(PgVectorError::DimensionsTooLarge { .. })
        ));
    }
}
