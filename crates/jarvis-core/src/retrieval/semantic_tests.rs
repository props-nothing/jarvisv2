// ------------------------------------------------------------------------------------------------
// Semantic similarity: an index, and never a truth
// ------------------------------------------------------------------------------------------------
//
// Every vector here is built from a **literal** array, so the reference is promoted to `'static` by const
// promotion rather than borrowed from a local. That is deliberate: a helper that leaked or cached values
// would make the tests depend on allocation order, and a `SemanticVector<'_>` borrowed from a temporary
// would not compile where the value outlives the statement that built it.

use super::*;
use crate::id::MemoryId;
use crate::memory::{EntityRef, MemoryType};

/// A normalized vector from one fixed model.
fn vector(values: &'static [f32], model: &'static str) -> SemanticVector<'static> {
    SemanticVector {
        provider: "openai-compatible",
        model,
        version: None,
        normalized: SemanticNormalization::Normalized,
        values,
    }
}

/// A lookup holding nothing, which is the state before any index exists.
fn holding_nothing() -> impl Fn(&MemoryId) -> Option<SemanticVector<'static>> {
    |_| None
}

/// A lookup holding one memory's vector, and none for any other.
fn holding(
    id: MemoryId,
    values: &'static [f32],
    model: &'static str,
) -> impl Fn(&MemoryId) -> Option<SemanticVector<'static>> {
    move |asked| (*asked == id).then(|| vector(values, model))
}

/// A lookup returning one given vector for every memory, for the incompatibility cases.
fn holding_any(
    held: SemanticVector<'static>,
) -> impl Fn(&MemoryId) -> Option<SemanticVector<'static>> {
    move |_| Some(held)
}

/// **A query with no embedding index scores the semantic signal zero, and says so.**
///
/// The state a caller reaches by not wiring embeddings at all. The signal is present in the table and its
/// value is zero, and the *reason* is reported — which is the distinction that keeps the table honest: a
/// zero because no index exists is not the same answer as a zero because the memory was ranked and lost.
#[test]
fn a_query_without_an_index_produces_no_semantic_score() {
    let context = ScoringContext::without_semantics();
    assert!(!context.has_semantics());
    let outcome = context.semantic_outcome(&MemoryId::new());
    assert_eq!(outcome.score(), 0);
    assert_eq!(outcome.absence(), Some(SemanticAbsence::NoIndex));

    // And the signal row is zero rather than missing, so a total is over the same nine signals either way.
    let record = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![EntityRef::confirmed(entity())],
    );
    let signals = signals_for(&record, &query().with_text("coffee"), &context);
    assert_eq!(signals.semantic, 0);
    assert_eq!(signals.all().len(), SIGNAL_COUNT);
}

/// **A memory with no vector is reported as absent, not as dissimilar.**
///
/// The case that makes an embedding an index rather than a requirement: a memory stored before the model
/// existed, or one the index skipped, is still retrievable and still scores on the other eight signals. What
/// it must not read as is "this memory is semantically unlike the question", because that is a claim about
/// its content rather than about the index.
#[test]
fn a_memory_with_no_vector_is_reported_as_absent_rather_than_dissimilar() {
    let context = ScoringContext::with_semantics(vector(&[1.0, 0.0], "m"), holding_nothing());
    let outcome = context.semantic_outcome(&MemoryId::new());
    assert_eq!(outcome.score(), 0);
    assert_eq!(outcome.absence(), Some(SemanticAbsence::NoMemoryVector));

    // An index holding a *different* memory is the same state for this one, which is why the lookup is a
    // lookup rather than a single vector.
    let held = MemoryId::new();
    let context =
        ScoringContext::with_semantics(vector(&[1.0, 0.0], "m"), holding(held, &[1.0, 0.0], "m"));
    assert_eq!(
        context.semantic_outcome(&MemoryId::new()).absence(),
        Some(SemanticAbsence::NoMemoryVector)
    );
}

/// **A vector from a different model, provider, or version is refused, and the field is named.**
///
/// The guard the whole type exists for. A cosine over two vectors from different models is a number with no
/// meaning that still lands in the usual range, so without this check the failure is not an error — it is a
/// slightly worse ranking, which is the kind of defect that survives review.
#[test]
fn a_vector_from_another_model_is_refused_and_names_the_field() {
    let id = MemoryId::new();
    let query_vector = vector(&[1.0, 0.0], "text-embedding-3-small");

    let mut other_model = vector(&[1.0, 0.0], "text-embedding-3-large");
    let outcome = ScoringContext::with_semantics(query_vector, holding_any(other_model))
        .semantic_outcome(&id);
    assert_eq!(
        outcome.absence(),
        Some(SemanticAbsence::Incomparable("model"))
    );
    assert_eq!(outcome.score(), 0);

    other_model.provider = "another-provider";
    let outcome = ScoringContext::with_semantics(query_vector, holding_any(other_model))
        .semantic_outcome(&id);
    assert_eq!(
        outcome.absence(),
        Some(SemanticAbsence::Incomparable("provider")),
        "the provider is compared before the model"
    );

    let mut other_version = vector(&[1.0, 0.0], "text-embedding-3-small");
    other_version.version = Some("2024-02");
    let outcome = ScoringContext::with_semantics(query_vector, holding_any(other_version))
        .semantic_outcome(&id);
    assert_eq!(
        outcome.absence(),
        Some(SemanticAbsence::Incomparable("version"))
    );
}

/// **Two vectors of different lengths are refused before the arithmetic.**
///
/// A dot product over two vectors of different lengths truncates to the shorter one without complaint, which
/// is the silent wrong answer a metadata guard exists to prevent. The check is against the vectors' own
/// lengths rather than against a declared dimension, because a declared dimension could contradict the
/// vector it describes.
#[test]
fn vectors_of_different_lengths_are_refused() {
    let id = MemoryId::new();
    let context = ScoringContext::with_semantics(
        vector(&[1.0, 0.0, 0.0], "m"),
        holding(id, &[1.0, 0.0], "m"),
    );

    let outcome = context.semantic_outcome(&id);
    assert_eq!(outcome.score(), 0);
    assert_eq!(outcome.absence(), Some(SemanticAbsence::LengthMismatch));
}

/// **A zero-magnitude vector refuses a comparison rather than dividing by zero.**
///
/// An all-zero embedding has no direction, so there is no angle between it and anything. Reporting a score
/// would require inventing a denominator, and a `NaN` that reached a total would poison a whole ranking
/// rather than one signal.
#[test]
fn a_zero_magnitude_vector_refuses_a_comparison() {
    let id = MemoryId::new();

    // The query is zero, with the normalization undeclared so the division happens at all.
    let mut query_vector = vector(&[0.0, 0.0], "m");
    query_vector.normalized = SemanticNormalization::Unknown;
    let context = ScoringContext::with_semantics(query_vector, holding(id, &[1.0, 0.0], "m"));
    assert_eq!(
        context.semantic_outcome(&id).absence(),
        Some(SemanticAbsence::ZeroMagnitude)
    );

    // The memory is zero.
    let mut memory_vector = vector(&[0.0, 0.0], "m");
    memory_vector.normalized = SemanticNormalization::Unknown;
    let mut query_vector = vector(&[1.0, 0.0], "m");
    query_vector.normalized = SemanticNormalization::Unknown;
    let context = ScoringContext::with_semantics(query_vector, holding_any(memory_vector));
    assert_eq!(
        context.semantic_outcome(&id).absence(),
        Some(SemanticAbsence::ZeroMagnitude)
    );
}

/// **Identical directions score full, a partial angle scores in between, and an opposed pair scores zero.**
///
/// The arithmetic's own sanity, on values whose cosine is known by construction rather than by computing it
/// twice. Two things this pins down:
///
/// 1. **A negative cosine scores zero, not a negative signal.** A component score is a magnitude out of
///    `SIGNAL_SCALE`, so letting an opposed vector pull a total *down* is something no other signal can do
///    and the weights are not shaped for it.
/// 2. **The value is the rounded cosine in thousandths.** An orthogonal pair scores exactly zero, and the
///    reason is recorded — a distinction the score alone cannot carry, since a missing vector, an
///    incomparable pair, and a genuine orthogonal result all produce zero.
#[test]
fn the_score_is_the_cosine_and_negatives_are_zero() {
    let id = MemoryId::new();
    let outcome = |query_values: &'static [f32], memory_values: &'static [f32]| {
        let mut query_vector = vector(query_values, "m");
        query_vector.normalized = SemanticNormalization::Unknown;
        ScoringContext::with_semantics(query_vector, holding(id, memory_values, "m"))
            .semantic_outcome(&id)
    };

    assert_eq!(
        outcome(&[1.0, 0.0], &[1.0, 0.0]).score(),
        SIGNAL_SCALE,
        "identical directions must score full"
    );
    assert_eq!(
        outcome(&[1.0, 0.0], &[0.5, 0.866_025_4]).score(),
        SIGNAL_SCALE / 2,
        "a sixty-degree angle has cosine one half, so it must score half the scale"
    );
    let orthogonal = outcome(&[1.0, 0.0], &[0.0, 1.0]);
    assert_eq!(
        orthogonal.score(),
        0,
        "an orthogonal pair has no similarity"
    );
    assert_eq!(
        orthogonal.absence(),
        Some(SemanticAbsence::NoSimilarity),
        "a zero from real arithmetic must be distinguishable from nothing being compared"
    );
    let opposed = outcome(&[1.0, 0.0], &[-1.0, 0.0]);
    assert_eq!(
        opposed.score(),
        0,
        "an opposed vector must not score negatively"
    );
    assert_eq!(opposed.absence(), Some(SemanticAbsence::NoSimilarity));
}

/// **An unnormalized vector is divided by its magnitude, or an unrelated vector scores full.**
/// **An unnormalized vector is divided by its magnitude, or an unrelated vector scores full.**
///
/// The concrete failure the `Normalized` default prevents. Without the division a **raw dot product** is
/// used, and a dot product scales with both vectors' lengths: `[2, 0]` dotted with `[10, 0]` is 20, which
/// exceeds the scale and clamps to full marks, so any long pair pointing the same way reads as a perfect
/// match. The cosine does not — it divides by both magnitudes, so 20/20 is exactly 1. This test asserts the
/// division happens by asserting that magnitude does not change the answer.
#[test]
fn an_unnormalized_vector_is_divided_by_its_magnitude() {
    const LONG: [f32; 2] = [10.0, 0.0];
    const LONG_ORTHOGONAL: [f32; 2] = [0.0, 10.0];
    const SHORT: [f32; 2] = [2.0, 0.0];
    let id = MemoryId::new();
    let undeclared = |memory_values: &'static [f32]| {
        let mut memory_vector = vector(memory_values, "m");
        memory_vector.normalized = SemanticNormalization::Unknown;
        let mut query_vector = vector(&SHORT, "m");
        query_vector.normalized = SemanticNormalization::Unknown;
        ScoringContext::with_semantics(query_vector, holding_any(memory_vector))
            .semantic_outcome(&id)
            .score()
    };

    assert_eq!(
        undeclared(&LONG),
        SIGNAL_SCALE,
        "the same direction must score full regardless of magnitude"
    );
    assert_eq!(
        undeclared(&LONG_ORTHOGONAL),
        0,
        "a long orthogonal vector must not score full, which is what skipping the division would do"
    );
}

/// **A semantic match outranks the same memory without a vector, and the reason names the meaning.**
///
/// The end-to-end claim of this slice: the signal reaches the ranking and changes an order. Two memories
/// with **identical text and source** — so the other eight signals are equal — are ranked with one carrying
/// a vector that matches the query's direction, and the one with the vector must win. The question shares no
/// words with either memory, so only the semantic term can separate them.
#[test]
fn a_semantic_match_outranks_the_same_memory_without_a_vector() {
    let first = memory(
        "A claim about coffee",
        MemoryType::Preference,
        vec![EntityRef::confirmed(entity())],
    );
    let second = memory(
        "A claim about coffee",
        MemoryType::Preference,
        vec![EntityRef::confirmed(entity())],
    );
    let asked = query().with_text("something else entirely");
    let context = ScoringContext::with_semantics(
        vector(&[1.0, 0.0], "m"),
        holding(first.id(), &[1.0, 0.0], "m"),
    );

    let ranked = rank(
        &[first.clone(), second.clone()],
        &asked,
        &context,
        &SIGNAL_WEIGHTS,
    );

    // The two memories are located by identifier and their **measured** scores asserted, rather than
    // asserting that `scored()[0]` is `first`. The reason is the tie-break: two memories with equal totals
    // are ordered by memory identifier, which is a v7 UUID generated at fixture construction, so a
    // position-based assertion is partly asserting which fixture happened to be built first. Asserting the
    // scores themselves — full for the memory with a matching vector, zero for the one without, and a total
    // differing by exactly the semantic weight — tests the mechanism instead of the fixture's id order.
    let winner = ranked
        .scored()
        .iter()
        .find(|scored| scored.record().id() == first.id())
        .map(ScoredMemory::signals);
    assert_eq!(
        winner.map(|signals| signals.semantic),
        Some(SIGNAL_SCALE),
        "the memory with a matching vector must score full on the semantic signal"
    );
    let loser = ranked
        .scored()
        .iter()
        .find(|scored| scored.record().id() == second.id())
        .map(ScoredMemory::signals);
    assert_eq!(
        loser.map(|signals| signals.semantic),
        Some(0),
        "the same memory without a vector must score zero, so the two are separated by the signal"
    );
    // And the measured scores are what put it first: the total differs by exactly the semantic weight.
    let first_total = ranked.scored()[0].total();
    let second_total = ranked.scored()[1].total();
    assert_eq!(
        first_total - second_total,
        u32::from(SIGNAL_WEIGHTS.semantic),
        "the only difference between the two memories is the semantic contribution"
    );
    assert_eq!(
        ranked.scored()[0].reason(),
        SelectionReason::SemanticSimilarity
    );
    assert!(ranked.scored()[0].reason().is_a_match());
}

/// **Two orders in one test, because they answer two questions.**
///
/// The **reason** a user is told prefers a match present in the text over one that is inferred, so a memory
/// whose words match is reported as a keyword match even when its meaning matches too. The **weight** keeps
/// the document's order, where semantic similarity sits above entity overlap, so a memory matching only by
/// meaning outranks one matching only by a shared entity. Collapsing the two would make one of them wrong.
///
/// Both fixtures are built so the *other* signals are equal, which is what makes each half a test of one
/// thing. The first query names no entity, so `exact_identifier` cannot fire and the two full-scale signals
/// are keyword and semantic. The second gives both memories exactly one of the query's two entities, so
/// their overlap scores are equal by construction and only the semantic weight can separate them.
#[test]
fn the_reason_prefers_text_and_the_weight_follows_the_document() {
    // A memory whose text matches AND whose meaning matches: the reason must be the text.
    let both = memory(
        "dark roast coffee",
        MemoryType::Preference,
        vec![EntityRef::confirmed(entity())],
    );
    let asked = query().with_text("dark roast coffee");
    let context = ScoringContext::with_semantics(
        vector(&[1.0, 0.0], "m"),
        holding(both.id(), &[1.0, 0.0], "m"),
    );
    let signals = signals_for(&both, &asked, &context);
    assert_eq!(
        signals.keyword, SIGNAL_SCALE,
        "the fixture must score full on keyword, or the precedence is untested"
    );
    assert_eq!(
        signals.semantic, SIGNAL_SCALE,
        "the fixture must score full on semantic too, so the two are equal and precedence decides"
    );
    assert_eq!(
        signals.exact_identifier, 0,
        "the query names no entity, so this signal cannot fire"
    );
    assert_eq!(
        reason_for(&signals, &SIGNAL_WEIGHTS),
        SelectionReason::KeywordMatch,
        "a match present in the text must be reported over an inferred one"
    );

    // Two memories each sharing one of the query's two entities: equal overlap, and only meaning differs.
    let named = entity();
    let other_named = entity();
    let entity_memory = memory(
        "some unrelated words",
        MemoryType::Semantic,
        vec![EntityRef::confirmed(named)],
    );
    let meaning_memory = memory(
        "entirely different words",
        MemoryType::Semantic,
        vec![EntityRef::confirmed(other_named)],
    );
    let asked = query()
        .with_entities(vec![named, other_named])
        .with_text("lives in rotterdam");
    let context = ScoringContext::with_semantics(
        vector(&[1.0, 0.0], "m"),
        holding(meaning_memory.id(), &[1.0, 0.0], "m"),
    );
    let ranked = rank(
        &[entity_memory.clone(), meaning_memory.clone()],
        &asked,
        &context,
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        ranked
            .scored()
            .iter()
            .map(|scored| scored.signals().entity_overlap)
            .collect::<Vec<_>>(),
        vec![SIGNAL_SCALE / 2, SIGNAL_SCALE / 2],
        "each memory must share exactly one of the two named entities"
    );
    // Bound through a local so this is a comparison of two values rather than a constant assertion, which
    // the linter refuses for good reason: a constant comparison is decided at compile time and a reader
    // cannot tell whether it was ever checked.
    let semantic_weight = SIGNAL_WEIGHTS.semantic;
    let overlap_weight = SIGNAL_WEIGHTS.entity_overlap;
    assert!(
        semantic_weight > overlap_weight,
        "the document lists semantic similarity above entity overlap"
    );
    assert_eq!(
        ranked.scored()[0].record().id(),
        meaning_memory.id(),
        "a semantic-only match must outrank an entity-only match, on the weights"
    );
}
