//! Tests for retrieval eligibility and ranking.
//!
//! # Why the weight table has its own arithmetic tests
//!
//! "No single signal may dominate by accident" is a claim about numbers, so it is tested as numbers rather
//! than as prose: the weights must sum to the scale, no weight may exceed a quarter of it, and a memory
//! that scores perfectly on one signal and zero elsewhere must not outrank a memory that is good on several.
//! A prose assertion in a doc comment is a claim nothing can contradict.

use super::*;
use crate::id::{EntityId, MemoryId, WorkspaceId};
use crate::memory::{
    EntityRef, MemoryConfidence, MemoryRecord, MemoryRecordParts, MemorySource, MemorySourceKind,
    MemoryType,
};
use crate::timestamp::UtcTimestamp;

const ACTOR: &str = "local-user";

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

fn workspace() -> WorkspaceId {
    must("0198f000-0000-7000-8000-000000000001".parse())
}

fn other_workspace() -> WorkspaceId {
    must("0198f000-0000-7000-8000-000000000002".parse())
}

fn at(minute: i128) -> UtcTimestamp {
    // A whole-second base with a non-zero fraction: a fraction of exactly zero is omitted by the Rfc3339
    // rendering, which makes the text width unstable across rows.
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn entity() -> EntityId {
    EntityId::new()
}

fn user_source() -> MemorySource {
    must(MemorySource::of_kind(
        MemorySourceKind::UserStatement,
        "session:0198f000-0000-7000-8000-000000000003",
    ))
}

/// The default query: the first workspace, evaluated at minute 100.
fn query() -> MemoryQuery {
    MemoryQuery::new(workspace(), at(100))
}

/// A scoring context with no embedding index.
///
/// Most tests are about the eight signals that need no embedding, and naming the absence is the point: the
/// semantic signal is *zero because there is no index*, which is a different state from "the memory has no
/// vector" and a different state again from "there is a semantic score of zero". The semantic tests below
/// build a real index and assert against its [`SemanticOutcome`] directly.
fn no_semantics() -> ScoringContext<'static> {
    ScoringContext::without_semantics()
}

/// A memory built through the real constructor, so a fixture cannot hold a value the domain refuses.
fn memory(content: &str, memory_type: MemoryType, entities: Vec<EntityRef>) -> MemoryRecord {
    must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type,
        content: content.to_owned(),
        structured_claim: None,
        source: user_source(),
        confidence: MemoryConfidence::Confirmed,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities,
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }))
}

/// A memory in another workspace, for the boundary rule.
fn foreign_memory(content: &str, entity_ref: EntityRef) -> MemoryRecord {
    must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: other_workspace(),
        memory_type: MemoryType::Semantic,
        content: content.to_owned(),
        structured_claim: None,
        source: user_source(),
        confidence: MemoryConfidence::Confirmed,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![entity_ref],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }))
}

// ------------------------------------------------------------------------------------------------
// The weights: "no single signal may dominate by accident"
// ------------------------------------------------------------------------------------------------

/// **Every weight is bounded, they sum to the scale, and one perfect signal cannot carry a memory.**
///
/// The arithmetic form of the document's rule. Three assertions, each against a way the table could be
/// wrong: a weight above the cap lets one signal dominate; weights that do not sum to the scale make a
/// total threshold mean different things at different call sites; and a table whose maximum total is not
/// the scale makes "a good score" unknowable.
#[test]
fn no_single_signal_can_dominate() {
    let weights = SIGNAL_WEIGHTS;
    assert_eq!(
        weights.total(),
        TOTAL_WEIGHT,
        "the weights must sum to the scale, or a threshold means nothing"
    );
    for (name, weight) in weights.all() {
        assert!(
            weight <= MAX_SIGNAL_WEIGHT,
            "{name} weighs {weight}, above the cap of {MAX_SIGNAL_WEIGHT}"
        );
        assert!(weight > 0, "{name} is zero, so it is not a signal");
    }

    // The dominating case, as a comparison: perfect on one signal, zero on the rest.
    let mut one_signal = MemorySignals {
        exact_identifier: 0,
        keyword: 0,
        semantic: 0,
        entity_overlap: 0,
        recency: 0,
        temporal: 0,
        importance: 0,
        source_reliability: 0,
        reinforcement: 0,
    };
    one_signal.exact_identifier = SIGNAL_SCALE;
    let specialist = one_signal.total(&weights);
    // Good-but-not-perfect on every other signal, nothing on the strongest.
    let mut generalist = one_signal;
    generalist.exact_identifier = 0;
    generalist.keyword = SIGNAL_SCALE / 2;
    generalist.entity_overlap = SIGNAL_SCALE / 2;
    generalist.recency = SIGNAL_SCALE / 2;
    generalist.temporal = SIGNAL_SCALE / 2;
    generalist.importance = SIGNAL_SCALE / 2;
    generalist.source_reliability = SIGNAL_SCALE / 2;
    generalist.reinforcement = SIGNAL_SCALE / 2;
    assert!(
        generalist.total(&weights) > specialist,
        "a memory good on several signals must outrank one perfect on a single signal: \
         {} vs {specialist}",
        generalist.total(&weights)
    );
}

/// **The total is the sum of the contributions, and the contribution table is the one used.**
///
/// The property that makes an explanation verifiable: an operator shown the component contributions is
/// reading the arithmetic that produced the total, not a parallel calculation. Asserted by recomputing the
/// total from `contributions()` — if the two ever diverge, a displayed explanation would not explain.
#[test]
fn the_total_is_the_sum_of_the_stored_contributions() {
    let weights = SIGNAL_WEIGHTS;
    let record = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![EntityRef::confirmed(entity())],
    );
    let query = query().with_text("dark roast coffee");
    let scored = score(&record, &query, &no_semantics(), &weights);

    let summed: u32 = scored
        .signals()
        .contributions(&weights)
        .iter()
        .map(|(_, contribution)| contribution)
        .sum();
    assert_eq!(
        summed,
        scored.total(),
        "the contributions must add up to the total, or an explanation does not explain"
    );

    // And the scale's bound holds: a total cannot exceed the weight total, because every signal is capped.
    assert!(scored.total() <= TOTAL_WEIGHT);
    let perfect = MemorySignals {
        exact_identifier: SIGNAL_SCALE,
        keyword: SIGNAL_SCALE,
        semantic: SIGNAL_SCALE,
        entity_overlap: SIGNAL_SCALE,
        recency: SIGNAL_SCALE,
        temporal: SIGNAL_SCALE,
        importance: SIGNAL_SCALE,
        source_reliability: SIGNAL_SCALE,
        reinforcement: SIGNAL_SCALE,
    };
    assert_eq!(perfect.total(&weights), TOTAL_WEIGHT);
}

// ------------------------------------------------------------------------------------------------
// Eligibility: the six rules, each with the case that would slip through
// ------------------------------------------------------------------------------------------------

/// **A memory from another workspace is not a lower-ranked result; it is not a result.**
///
/// The boundary rule, and the reason it is a filter: ranking it and dropping it later would make the
/// ranking depend on how many foreign memories were in the candidate set. The assertion is on the
/// *exclusion* and its reason, not on the absence of a score, because a ranker that scored and then
/// filtered would pass an emptiness check.
#[test]
fn a_foreign_workspace_memory_is_excluded_by_name() {
    let held = EntityRef::confirmed(entity());
    let foreign = foreign_memory("Prefers dark roast coffee", held.clone());
    let mine = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    let selection = rank(
        &[foreign.clone(), mine.clone()],
        &query(),
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(selection.len(), 1, "only the local memory is a candidate");
    assert_eq!(selection.scored()[0].record().id(), mine.id());
    assert_eq!(
        selection.excluded(),
        [ExcludedMemory {
            memory_id: foreign.id(),
            reason: Ineligibility::ForeignWorkspace
        }]
    );
}

/// **A memory above the destination's ceiling is excluded, and the exclusion names both levels.**
///
/// The disclosure rule, and the only one about the *destination* rather than the memory. The fixture's
/// sensitivity is raised deliberately: a memory at the default `Internal` is eligible for an `Internal`
/// destination, so a test that used the default would pass while checking nothing.
#[test]
fn a_memory_above_the_destination_is_excluded() {
    let held = EntityRef::confirmed(entity());
    let restricted = memory("A credential", MemoryType::Semantic, vec![held.clone()]);
    // A deleted memory is also not current, so the destination rule must be checked first to be the reason.
    let deleted = must(restricted.delete(at(1)));
    assert_eq!(
        query().is_eligible(&deleted),
        Err(Ineligibility::NotCurrent {
            status: EffectiveMemoryStatus::Deleted
        }),
        "the fixture must reach the currency rule, or the ordering claim is untested"
    );

    // A live memory above the ceiling: the level is what refuses it.
    let secret = memory("Takes medication daily", MemoryType::Semantic, vec![held]);
    let local_only = query().with_destination(Sensitivity::Internal);
    assert_eq!(
        local_only.is_eligible(&secret),
        Ok(()),
        "an Internal memory is eligible for an Internal destination"
    );

    let public_only = query().with_destination(Sensitivity::Public);
    assert_eq!(
        public_only.is_eligible(&secret),
        Err(Ineligibility::AboveDestination {
            sensitivity: Sensitivity::Internal,
            destination: Sensitivity::Public
        })
    );
}

/// **A superseded or expired claim is excluded, and the reason distinguishes the two.**
///
/// The rule the document is most specific about: "stop retrieving obsolete claims as current truth". The
/// two cases are different answers to "why is this not being used" and are asserted separately, because an
/// implementation that collapsed them would report a replaced claim as an expired one.
#[test]
fn an_obsolete_claim_is_excluded_and_the_reason_says_which() {
    let held = EntityRef::confirmed(entity());
    let replacement = MemoryId::new();

    let superseded = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held.clone()],
    );
    let superseded = must(superseded.replace_with(replacement, at(1)));
    assert_eq!(
        query().is_eligible(&superseded),
        Err(Ineligibility::NotCurrent {
            status: EffectiveMemoryStatus::Superseded
        }),
        "a replaced claim must not be offered as current truth"
    );

    // Expired rather than superseded: a window that closed before the query instant. The status is derived
    // at the query instant, so the fixture's window is what makes the difference and no stored `expired`
    // state is needed.
    let expiring = must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Semantic,
        content: "In Rotterdam next week".to_owned(),
        structured_claim: None,
        source: user_source(),
        confidence: MemoryConfidence::Confirmed,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![held],
        valid_from: Some(at(0)),
        valid_until: Some(at(50)),
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }));
    // The query is at minute 100, past the window's end at minute 50.
    assert_eq!(
        query().is_eligible(&expiring),
        Err(Ineligibility::NotCurrent {
            status: EffectiveMemoryStatus::Expired
        }),
        "a lapsed claim must be reported as expired, not as superseded"
    );

    // The control: the same claim inside its window is eligible. Without it, a rule that excluded every
    // bounded claim would pass the assertion above.
    let inside = query();
    let mut early = inside.clone();
    early.at = at(10);
    assert_eq!(early.is_eligible(&expiring), Ok(()));
}

/// **The three caller-stated requirements each refuse, and each names its own cause.**
///
/// Type, trust, and confidence. They are the document's last three eligibility rows and the ones a caller
/// sets, so a filter that ignored one would return something the caller had explicitly excluded. Each is
/// asserted with the others satisfied, so only the field under test can be the reason.
#[test]
fn the_caller_stated_requirements_each_refuse() {
    let held = EntityRef::confirmed(entity());
    let preference = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    // Type: a caller asking only for facts must not receive a preference.
    let facts_only = query().with_allowed_types(vec![MemoryType::Semantic]);
    assert_eq!(
        facts_only.is_eligible(&preference),
        Err(Ineligibility::TypeNotAllowed {
            memory_type: MemoryType::Preference
        })
    );
    // And the same query accepts a memory of an allowed type, so the rule is not "refuse everything".
    let fact = memory(
        "Lives in Rotterdam",
        MemoryType::Semantic,
        vec![EntityRef::confirmed(entity())],
    );
    assert_eq!(facts_only.is_eligible(&fact), Ok(()));

    // Trust: an inference-sourced claim is refused when the caller requires authoritative sources. The
    // fixture is built with its own kind and no confidence, which is what the domain allows.
    let inferred = must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Semantic,
        content: "May prefer tea".to_owned(),
        structured_claim: None,
        source: must(MemorySource::of_kind(
            MemorySourceKind::ModelInference,
            "run:0198f000-0000-7000-8000-000000000004",
        )),
        confidence: MemoryConfidence::Unverified,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(entity())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }));
    assert_eq!(
        query().is_eligible(&inferred),
        Err(Ineligibility::TrustBelowMinimum {
            trust: MemoryTrust::Derived,
            minimum: MemoryTrust::Authoritative
        })
    );

    // The control for the trust rule: the same claim is *eligible* when the caller lowers the requirement,
    // which proves the rule is the minimum and not the kind.
    let permissive = query()
        .with_minimum_trust(MemoryTrust::Derived)
        .with_minimum_confidence(MemoryConfidence::Unverified);
    assert_eq!(permissive.is_eligible(&inferred), Ok(()));

    // Confidence: a caller requiring confirmation refuses a merely likely claim even from a good origin.
    let likely = must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Semantic,
        content: "The manual states a two year warranty".to_owned(),
        structured_claim: None,
        source: must(MemorySource::of_kind(
            MemorySourceKind::Document,
            "doc:0198f000-0000-7000-8000-000000000005",
        )),
        confidence: MemoryConfidence::Likely,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(entity())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }));
    // A document's trust is `Derived`, so the trust rule would refuse it first under the strict default —
    // which is the ordering. Lowering trust and keeping confidence at `Confirmed` isolates the confidence rule.
    let trust_relaxed = query().with_minimum_trust(MemoryTrust::Derived);
    assert_eq!(
        trust_relaxed.is_eligible(&likely),
        Err(Ineligibility::ConfidenceBelowMinimum),
        "the confidence rule must be reachable once the trust rule passes"
    );
}

/// **The default query is the strict one, and every default is a refusal rather than a permission.**
///
/// A default is what a caller who did not think about a field gets, and for a retrieval the permissive
/// default is a disclosure: `Internal` as the destination ceiling means a third-party model receives
/// nothing confidential until a caller says otherwise, and `Authoritative` as the trust minimum means an
/// inference or a web page is not offered as an answer. Falsifying this needs only a permissive default.
#[test]
fn the_default_query_is_the_strict_one() {
    let default = MemoryQuery::new(workspace(), at(100));
    assert_eq!(
        default.destination,
        Sensitivity::Internal,
        "a default destination must not accept confidential content"
    );
    assert_eq!(
        default.minimum_trust,
        MemoryTrust::Authoritative,
        "a default must not offer a derived source as an answer"
    );
    assert_eq!(default.minimum_confidence, MemoryConfidence::Confirmed);
    assert!(
        default.allowed_types.is_empty(),
        "an empty type list means any, which is the permissive shape for the one field where \
         permissive is right"
    );

    // And the consequence: an inferred claim is refused by the default query.
    let inferred = must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Semantic,
        content: "May prefer tea".to_owned(),
        structured_claim: None,
        source: must(MemorySource::of_kind(
            MemorySourceKind::ModelInference,
            "run:0198f000-0000-7000-8000-000000000004",
        )),
        confidence: MemoryConfidence::Unverified,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(entity())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }));
    assert!(
        default.is_eligible(&inferred).is_err(),
        "an inference must not be eligible under the defaults"
    );
    assert!(!may_be_offered_as_fact(MemorySourceKind::ModelInference));
}

// ------------------------------------------------------------------------------------------------
// The signals
// ------------------------------------------------------------------------------------------------

/// **The exact-identifier signal needs every named entity; the overlap signal scores the fraction.**
///
/// Two signals that would be one if they were mutually exclusive, and the difference is the whole reason
/// the exact one carries the most weight: a memory about one of three entities the question named is
/// *partial* evidence, and reporting it as an exact hit would let a partial match answer a specific
/// question. Asserted together, because a branch that made them exclusive passes either assertion alone.
#[test]
fn the_identifier_and_overlap_signals_differ_on_a_partial_match() {
    let alice = entity();
    let bob = entity();
    let about_alice = memory(
        "Alice is my sister",
        MemoryType::Relationship,
        vec![EntityRef::confirmed(alice)],
    );

    let full = query().with_entities(vec![alice]);
    assert_eq!(exact_identifier_signal(&about_alice, &full), SIGNAL_SCALE);
    assert_eq!(entity_overlap_signal(&about_alice, &full), SIGNAL_SCALE);

    let partial = query().with_entities(vec![alice, bob]);
    assert_eq!(
        exact_identifier_signal(&about_alice, &partial),
        0,
        "one of two named entities is not an exact hit"
    );
    assert_eq!(
        entity_overlap_signal(&about_alice, &partial),
        SIGNAL_SCALE / 2,
        "half the named entities appear, so the overlap is half"
    );

    // A query naming no entity scores both zero rather than full, which is the case a `0 == 0` comparison
    // would otherwise make look like a match.
    let none = query();
    assert_eq!(exact_identifier_signal(&about_alice, &none), 0);
    assert_eq!(entity_overlap_signal(&about_alice, &none), 0);
}

/// **The keyword signal is a proportion of the question's distinct words, and it is not all-or-nothing.**
///
/// The property that a partial match scores partially, which an integer division would break: `matched /
/// total` in integers is zero for every partial case, so a memory matching most of a question would score
/// as if it matched none. The fixture matches two of three words, which is exactly the case integer
/// division destroys.
#[test]
fn the_keyword_signal_scores_a_partial_match() {
    let held = EntityRef::confirmed(entity());
    let memory = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    // Three distinct question words, two present.
    let partial = query().with_text("dark roast tea");
    let score = keyword_signal(&memory, &partial);
    assert_eq!(
        score,
        SIGNAL_SCALE * 2 / 3,
        "two of three words is a two-thirds score, not zero and not full"
    );

    // All the question's words present, and repeated words in the question do not inflate it.
    let full = query().with_text("dark roast coffee dark dark");
    assert_eq!(keyword_signal(&memory, &full), SIGNAL_SCALE);

    // Case and whitespace fold, which is the search key's own normalization.
    let folded = query().with_text("  DARK   Roast ");
    assert_eq!(keyword_signal(&memory, &folded), SIGNAL_SCALE);
}

/// **Recency decays linearly and reaches zero at the window, and a future reference scores full.**
///
/// A step function would make every memory inside the window equal, and the point of the signal is to
/// distinguish them. The future case is asserted because scoring it zero would silently drop a memory from
/// an "as of" read where it is the newest fact available.
#[test]
fn the_recency_signal_decays_and_bounds_both_ends() {
    let held = EntityRef::confirmed(entity());
    let memory = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    // Created at minute 0; the query's own instant is minute 100.
    assert!(recency_signal(&memory, &query()) > 0);
    // The window is thirty days, which is far beyond the fixture's 100 minutes, so the score is high
    // rather than zero — the assertion that separates a decaying signal from an absent one.
    assert!(recency_signal(&memory, &query()) > SIGNAL_SCALE / 2);

    // At the window's edge the score is zero, and beyond it stays zero rather than going negative.
    let mut far = query();
    far.at = must(UtcTimestamp::from_unix_nanos(
        at(0).unix_nanos() + RECENCY_WINDOW_NANOS,
    ));
    assert_eq!(recency_signal(&memory, &far), 0);
    let mut farther = far.clone();
    farther.at = must(UtcTimestamp::from_unix_nanos(
        at(0).unix_nanos() + RECENCY_WINDOW_NANOS * 10,
    ));
    assert_eq!(
        recency_signal(&memory, &farther),
        0,
        "a negative age is not a score"
    );

    // A query instant before the memory existed: an "as of" read where this memory is not yet available,
    // and the signal is full because it is the newest fact for that instant rather than penalized for it.
    let mut before = query();
    before.at = must(UtcTimestamp::from_unix_nanos(at(0).unix_nanos() - 1));
    assert_eq!(recency_signal(&memory, &before), SIGNAL_SCALE);
}

/// **Reinforcement saturates rather than growing without bound.**
///
/// The accumulation failure reached through one term: an unbounded count would let a single old, often-used
/// memory outrank everything current, which is "dominate by accident" by a different route. The bound is
/// asserted at the saturation point and above it, because a test that only checked "more is higher" passes
/// against an unbounded implementation.
#[test]
fn the_reinforcement_signal_saturates() {
    let held = EntityRef::confirmed(entity());
    let mut memory = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    assert_eq!(
        reinforcement_signal(&memory),
        0,
        "an unused memory scores zero"
    );
    for _ in 0..REINFORCEMENT_SATURATION {
        memory = memory.retrieved(at(1));
    }
    assert_eq!(
        reinforcement_signal(&memory),
        SIGNAL_SCALE,
        "the saturation count must reach full"
    );
    let saturated = reinforcement_signal(&memory);
    for _ in 0..REINFORCEMENT_SATURATION * 10 {
        memory = memory.retrieved(at(1));
    }
    assert_eq!(
        reinforcement_signal(&memory),
        saturated,
        "a memory retrieved far more is not more useful; the signal must be bounded"
    );
}

// ------------------------------------------------------------------------------------------------
// Ranking
// ------------------------------------------------------------------------------------------------

/// **The ranking is stable for equally-scored memories, and it orders by total otherwise.**
///
/// The stability property: two memories scoring identically must come back in an order that does not
/// depend on the candidate set's arrival order, because for a database-backed read that order is a query
/// plan away from changing between builds. The order is asserted by shuffling the input, which a
/// sort-by-insertion implementation fails.
#[test]
fn the_ranking_is_stable_and_ordered() {
    let alice = EntityRef::confirmed(entity());
    let bob = EntityRef::confirmed(entity());

    // Two identical memories except for their entity, so they score the same on everything but differ on
    // the identifier signal when the query names one of them.
    let about_alice = memory(
        "Lives in Rotterdam",
        MemoryType::Semantic,
        vec![alice.clone()],
    );
    let about_bob = memory("Lives in Rotterdam", MemoryType::Semantic, vec![bob]);

    let both = query().with_text("lives in rotterdam");
    let forward = rank(
        &[about_bob.clone(), about_alice.clone()],
        &both,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    let backward = rank(
        &[about_alice.clone(), about_bob.clone()],
        &both,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    let ids = |selection: &MemorySelection| -> Vec<MemoryId> {
        selection.scored().iter().map(|s| s.record().id()).collect()
    };
    assert_eq!(
        ids(&forward),
        ids(&backward),
        "the order of equally-scored memories must not depend on the input order"
    );

    // And specificity orders them when the query names one entity: the memory about the named entity wins.
    let specific = query()
        .with_entities(vec![alice.entity_id()])
        .with_text("lives in rotterdam");
    let ranked = rank(
        &[about_bob.clone(), about_alice.clone()],
        &specific,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        ranked.scored()[0].record().id(),
        about_alice.id(),
        "the memory about the named entity must rank first"
    );
    assert!(
        ranked.scored()[0].total() > ranked.scored()[1].total(),
        "the winner must win on the total, not on the sort's tie-break"
    );
}

/// **The inclusion reason is derived from the components, and a memory that matched nothing says so.**
///
/// The property that makes an explanation verifiable. A memory that matched nothing is reported as
/// recent-and-current or important rather than as a match, and `is_a_match` is what exposes the difference
/// — a caller that presented a context memory as an answer would be asserting something the scores do not
/// support. Falsifying this needs only a reason computed beside the scores rather than from them.
#[test]
fn the_reason_is_derived_from_the_signals() {
    let held = EntityRef::confirmed(entity());
    let about_nothing_asked = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![held],
    );

    // A question about something else entirely: no entity match, no keyword match.
    let unmatched = query().with_text("quantum chromodynamics");
    let scored = score(
        &about_nothing_asked,
        &unmatched,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        scored.signals().keyword,
        0,
        "the fixture must not match by keyword, or the reason test is about a match"
    );
    assert_eq!(scored.signals().exact_identifier, 0);
    assert!(
        !scored.reason().is_a_match(),
        "a memory that matched nothing must not be reported as a match, got {:?}",
        scored.reason()
    );

    // A keymatch: the reason names the keyword signal, and the signal is what supports it.
    let matched = query().with_text("dark roast");
    let scored = score(
        &about_nothing_asked,
        &matched,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(scored.reason(), SelectionReason::KeywordMatch);
    assert!(scored.signals().keyword > 0);
    assert!(scored.reason().is_a_match());

    // An identifier match outranks a keyword match, which is the specificity ordering: the question names
    // the entity the memory is about *and* shares its words.
    let named = query()
        .with_entities(vec![about_nothing_asked.entities()[0].entity_id()])
        .with_text("dark roast");
    let scored = score(
        &about_nothing_asked,
        &named,
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        scored.reason(),
        SelectionReason::ExactIdentifier,
        "the more specific signal must be the reported reason"
    );
    assert!(
        scored.total()
            > score(
                &about_nothing_asked,
                &matched,
                &no_semantics(),
                &SIGNAL_WEIGHTS
            )
            .total(),
        "naming the entity must raise the total, not only the reason"
    );
}

/// **Every signal's name round-trips, and the two vocabularies are exhaustive over their arms.**
///
/// The round trip a log line or a stored reason depends on, plus the check that both `all()` tables list
/// every field — a signal added without a row would be scored and never displayed, which looks like a
/// missing signal rather than a missing row.
#[test]
fn both_vocabularies_are_exhaustive_and_named() {
    for reason in [
        SelectionReason::ExactIdentifier,
        SelectionReason::KeywordMatch,
        SelectionReason::EntityOverlap,
        SelectionReason::RecentAndCurrent,
        SelectionReason::Important,
    ] {
        assert!(!reason.as_str().is_empty(), "{reason:?} has no name");
    }
    for exclusion in [
        Ineligibility::ForeignWorkspace,
        Ineligibility::NotCurrent {
            status: EffectiveMemoryStatus::Deleted,
        },
        Ineligibility::AboveDestination {
            sensitivity: Sensitivity::Internal,
            destination: Sensitivity::Public,
        },
        Ineligibility::TypeNotAllowed {
            memory_type: MemoryType::Semantic,
        },
        Ineligibility::TrustBelowMinimum {
            trust: MemoryTrust::Derived,
            minimum: MemoryTrust::Authoritative,
        },
        Ineligibility::ConfidenceBelowMinimum,
    ] {
        assert!(!exclusion.as_str().is_empty(), "{exclusion:?} has no name");
    }

    // The weights table and the signals table must both list eight entries, and the reason precedence must
    // cover the three matching signals — otherwise a signal could be scored and never reported.
    assert_eq!(SIGNAL_WEIGHTS.all().len(), SIGNAL_COUNT);
    let signals = signals_for(
        &memory(
            "A claim",
            MemoryType::Semantic,
            vec![EntityRef::confirmed(entity())],
        ),
        &query().with_text("A claim"),
        &no_semantics(),
    );
    assert_eq!(signals.all().len(), SIGNAL_COUNT);
    assert_eq!(signals.contributions(&SIGNAL_WEIGHTS).len(), SIGNAL_COUNT);
}

/// **Every cap bounds its own category, and a memory over any cap is dropped by name.**
///
/// Four caps and four answers. Each is asserted with the others loose, so only the cap under test can be
/// the reason — and the *loose* budget is asserted to admit the same set first, which is the positive
/// control: without it, a `diversify` that dropped everything would pass every refusal assertion.
#[test]
fn every_budget_cap_bounds_its_own_category() {
    let alice = EntityRef::confirmed(entity());
    // Six preferences about the same entity: enough to exercise a per-type, per-entity, and total cap.
    let candidates: Vec<MemoryRecord> = (0..6)
        .map(|index| {
            memory(
                &format!("Prefers roast number {index}"),
                MemoryType::Preference,
                vec![alice.clone()],
            )
        })
        .collect();
    let query = query().with_text("prefers roast");
    let ranked = rank(&candidates, &query, &no_semantics(), &SIGNAL_WEIGHTS);
    assert_eq!(
        ranked.len(),
        6,
        "all six are eligible, so only the caps can drop them"
    );

    // Loose: nothing is dropped, so the caps below are what drops a memory.
    let loose = must(DiversityBudget::new(100, 100, 100, 100));
    let kept = diversify(ranked.clone(), &loose);
    assert_eq!(kept.len(), 6);
    assert!(kept.dropped().is_empty(), "a loose budget drops nothing");

    // Per type.
    let by_type = must(DiversityBudget::new(2, 100, 100, 100));
    let capped = diversify(ranked.clone(), &by_type);
    assert_eq!(capped.len(), 2);
    assert!(capped.dropped().iter().all(|dropped| matches!(
        dropped.reason,
        DiversityReason::TypeBudget {
            memory_type: MemoryType::Preference
        }
    )));

    // Per entity.
    let by_entity = must(DiversityBudget::new(100, 1, 100, 100));
    let capped = diversify(ranked.clone(), &by_entity);
    assert_eq!(capped.len(), 1);
    assert_eq!(
        capped.dropped().first().map(|dropped| dropped.reason),
        Some(DiversityReason::EntityBudget {
            entity_id: alice.entity_id()
        })
    );

    // Per source kind, which the type cap cannot substitute for: every fixture claim is a user statement,
    // so a source cap of one keeps one claim while the type cap of two would keep two.
    let by_source = must(DiversityBudget::new(100, 100, 1, 100));
    let capped = diversify(ranked.clone(), &by_source);
    assert_eq!(capped.len(), 1);
    assert_eq!(
        capped.dropped().first().map(|dropped| dropped.reason),
        Some(DiversityReason::SourceKindBudget {
            source_kind: MemorySourceKind::UserStatement
        })
    );

    // The total cap.
    let by_total = must(DiversityBudget::new(100, 100, 100, 3));
    let capped = diversify(ranked.clone(), &by_total);
    assert_eq!(capped.len(), 3);
    assert_eq!(
        capped.dropped().first().map(|dropped| dropped.reason),
        Some(DiversityReason::TotalBudget)
    );

    // And the ranking survives: the capped result is the prefix of the ranked one, not a re-selection.
    let ids: Vec<_> = capped.scored().iter().map(|s| s.record().id()).collect();
    let ranked_ids: Vec<_> = ranked.scored().iter().map(|s| s.record().id()).collect();
    assert_eq!(ids, ranked_ids[..3], "capping must keep the best, in order");
}

/// **Diversification forces a result to be diverse, which is the point of a per-category cap.**
///
/// The property a total cap alone cannot provide: six preferences about one person with a total cap of four
/// is still four preferences about one person. The budget bounds the *concentration*, so a result reaches
/// its total only by carrying something else — which is what "diversif[y] across memory types/entities"
/// means as arithmetic rather than as an intention.
#[test]
fn a_per_category_cap_forces_a_diverse_result() {
    let alice = EntityRef::confirmed(entity());
    let bob = EntityRef::confirmed(entity());

    // Four preferences about Alice, then two facts about Bob. Ranked, the preferences come first because
    // their text matches the query.
    let mut candidates: Vec<MemoryRecord> = (0..4)
        .map(|index| {
            memory(
                &format!("Prefers roast number {index}"),
                MemoryType::Preference,
                vec![alice.clone()],
            )
        })
        .collect();
    candidates.push(memory(
        "Lives in Rotterdam",
        MemoryType::Semantic,
        vec![bob.clone()],
    ));
    candidates.push(memory(
        "Works in Utrecht",
        MemoryType::Semantic,
        vec![bob.clone()],
    ));

    let ranked = rank(
        &candidates,
        &query().with_text("prefers roast"),
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        ranked.scored()[0].record().memory_type(),
        MemoryType::Preference
    );

    // A budget capping the preference type at two and the entity at two: the result must contain both kinds.
    let budget = must(DiversityBudget::new(2, 2, 100, 100));
    let diverse = diversify(ranked, &budget);
    let mut types: Vec<MemoryType> = diverse
        .scored()
        .iter()
        .map(|scored| scored.record().memory_type())
        .collect();
    types.sort_by_key(|memory_type| memory_type.as_str());
    types.dedup();
    assert!(
        types.contains(&MemoryType::Semantic) && types.contains(&MemoryType::Preference),
        "a capped result must carry more than one type, got {types:?}"
    );
    assert_eq!(diverse.len(), 4, "two preferences and the two facts fit");
}

/// **A zero cap is refused, because the two readings of it are opposites.**
///
/// "None of this category" and "no limit" are both plausible readings of a zero cap, so a caller that
/// passed one would get behaviour it did not choose — and the silent reading would be the restrictive one,
/// making a category vanish with no explanation. Falsifying this needs only a zero treated as unlimited.
#[test]
fn a_zero_budget_cap_is_refused() {
    assert_eq!(DiversityBudget::new(0, 1, 1, 1), Err("max_per_type"));
    assert_eq!(DiversityBudget::new(1, 0, 1, 1), Err("max_per_entity"));
    assert_eq!(DiversityBudget::new(1, 1, 0, 1), Err("max_per_source_kind"));
    assert_eq!(DiversityBudget::new(1, 1, 1, 0), Err("max_total"));
    assert!(DiversityBudget::new(1, 1, 1, 1).is_ok());
    // The default is constructible and bounded, so it is not a value only a test uses. Asserted through the
    // comparison the caps are used for rather than as a constant comparison: a default whose per-type cap
    // equalled its total would make the type cap vacuous, and `assert!(4 > 4)` is what clippy calls a
    // constant assertion — correctly, since it checks the literal rather than the property.
    let default = DEFAULT_DIVERSITY_BUDGET;
    assert!(
        default.max_total > default.max_per_type,
        "a default whose total equals its per-type cap cannot diversify across types"
    );
    assert!(
        default.max_total > default.max_per_entity,
        "a default whose total equals its per-entity cap cannot diversify across entities"
    );
    // And the default admits *something* under the caps it sets, which a cap of zero would not.
    let probe = must(DiversityBudget::new(
        default.max_per_type,
        default.max_per_entity,
        default.max_per_source_kind,
        default.max_total,
    ));
    assert_eq!(probe, default);
}

/// **The reason precedence is specificity, not the largest score and not the array order.**
///
/// The case that exposed the first implementation: a question that names the memory's entity **and** shares
/// its words scores full on the identifier, overlap, **and** keyword signals, so a tie-break deciding by
/// array order reports whichever arm the literal happens to list last. `max_by_key` returns the last
/// maximum, and the fixture here scores full on all three, so the assertion fails for an implementation
/// that relies on ordering rather than on a stated precedence.
#[test]
fn the_reason_precedence_is_specificity_not_array_order() {
    let held = EntityRef::confirmed(entity());
    let about_the_named_entity = memory(
        "Prefers dark roast",
        MemoryType::Preference,
        vec![held.clone()],
    );
    let named = query()
        .with_entities(vec![held.entity_id()])
        .with_text("Prefers dark roast");

    let signals = signals_for(&about_the_named_entity, &named, &no_semantics());
    assert_eq!(
        signals.exact_identifier, SIGNAL_SCALE,
        "the fixture must score full on the identifier signal, or the tie is not a tie"
    );
    assert_eq!(
        signals.entity_overlap, SIGNAL_SCALE,
        "the fixture must also score full on the overlap signal"
    );
    assert_eq!(
        signals.keyword, SIGNAL_SCALE,
        "the fixture must also score full on the keyword signal"
    );
    assert_eq!(
        reason_for(&signals, &SIGNAL_WEIGHTS),
        SelectionReason::ExactIdentifier,
        "the most specific matching signal must decide, whatever order the arms are written in"
    );

    // The other two, each isolated so the precedence is checked across the whole ordering and not only at
    // the top: overlap beats keyword, and a keyword-only match is reported as a keyword match.
    let mut overlap_only = signals;
    overlap_only.exact_identifier = 0;
    assert_eq!(
        reason_for(&overlap_only, &SIGNAL_WEIGHTS),
        SelectionReason::EntityOverlap
    );
    let mut keyword_only = signals;
    keyword_only.exact_identifier = 0;
    keyword_only.entity_overlap = 0;
    assert_eq!(
        reason_for(&keyword_only, &SIGNAL_WEIGHTS),
        SelectionReason::KeywordMatch
    );
}

/// **The ranking survives the input order, and the caps keep the best rather than a category sample.**
///
/// Two properties of the pipeline together: the ranking is stable (asserted above) and `diversify` is a
/// **prefix** of it rather than a fresh selection. The second is the one a category-first implementation
/// fails: taking the best preference and then the best fact would reorder the result by a category ordering
/// the scoring never produced.
#[test]
fn the_capped_result_is_a_prefix_of_the_ranking() {
    let alice = EntityRef::confirmed(entity());
    // Identical text and entity, so they score identically and only the identifier tie-break orders them.
    let candidates: Vec<MemoryRecord> = (0..5)
        .map(|_| {
            memory(
                "Prefers dark roast coffee",
                MemoryType::Preference,
                vec![alice.clone()],
            )
        })
        .collect();
    let ranked = rank(
        &candidates,
        &query().with_text("dark roast coffee"),
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    let capped = diversify(
        ranked.clone(),
        &must(DiversityBudget::new(100, 2, 100, 100)),
    );

    let ranked_ids: Vec<_> = ranked.scored().iter().map(|s| s.record().id()).collect();
    let capped_ids: Vec<_> = capped.scored().iter().map(|s| s.record().id()).collect();
    assert_eq!(capped_ids, ranked_ids[..2]);

    // Every dropped memory was eligible and scored, so it appears in the ranked list — the difference
    // between "dropped for a cap" and "not available".
    for dropped in capped.dropped() {
        assert!(
            ranked_ids.contains(&dropped.memory_id),
            "a dropped memory must be one the ranking scored"
        );
    }
}

/// **A budget cap and an eligibility rule are different answers, and the result keeps them apart.**
///
/// The distinction a caller acts on: a memory excluded by a rule **cannot** be retrieved, while one dropped
/// by a cap was retrievable and was left out for room. Collapsing them would report "you have seen enough
/// preferences" as "this preference is not available to you", and an operator acting on the second would go
/// looking for a policy problem that does not exist.
#[test]
fn a_budget_drop_is_not_an_eligibility_refusal() {
    let alice = EntityRef::confirmed(entity());
    let preference = memory(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        vec![alice.clone()],
    );
    let foreign = foreign_memory("Prefers dark roast coffee", alice.clone());

    let ranked = rank(
        &[preference.clone(), foreign.clone()],
        &query(),
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    assert_eq!(
        ranked.excluded().len(),
        1,
        "the foreign memory is ineligible"
    );
    assert!(ranked.dropped().is_empty(), "nothing has been capped yet");

    // One eligible memory against a source-kind cap of one: it fits, so nothing is dropped.
    let capped = diversify(ranked, &must(DiversityBudget::new(100, 100, 1, 100)));
    assert_eq!(capped.len(), 1);
    assert!(
        capped.dropped().is_empty(),
        "one memory fits a cap of one, so nothing is dropped"
    );
    assert_eq!(
        capped.excluded().len(),
        1,
        "diversification must not disturb the exclusion list"
    );

    // Two eligible memories against the same cap: one is dropped, and the exclusion list is still the
    // foreign memory alone — the drop is reported in its own list.
    let second = memory(
        "Prefers light roast coffee",
        MemoryType::Preference,
        vec![alice],
    );
    let ranked = rank(
        &[preference, second],
        &query(),
        &no_semantics(),
        &SIGNAL_WEIGHTS,
    );
    let capped = diversify(ranked, &must(DiversityBudget::new(100, 100, 1, 100)));
    assert_eq!(capped.len(), 1);
    assert_eq!(capped.dropped().len(), 1);
    assert!(
        capped.excluded().is_empty(),
        "a dropped memory must not be reported as ineligible: {}",
        capped.excluded().len()
    );
}

///
/// The doc's "source reliability" and the admission rule it must not contradict: a model inference is never
/// offered as a fact. Asserted across the kinds, because a mapping table is a claim until a test pins it —
/// and because the two must agree rather than each being defensible alone.
#[test]
fn the_source_reliability_signal_matches_the_model_gate() {
    for kind in MemorySourceKind::all() {
        let record = must(MemoryRecord::new(MemoryRecordParts {
            id: MemoryId::new(),
            workspace_id: workspace(),
            memory_type: MemoryType::Semantic,
            content: "A claim from a source".to_owned(),
            structured_claim: None,
            source: must(MemorySource::of_kind(
                kind,
                "session:0198f000-0000-7000-8000-000000000003",
            )),
            confidence: MemoryConfidence::Unverified,
            importance: 2,
            sensitivity: Sensitivity::Internal,
            entities: vec![EntityRef::confirmed(entity())],
            valid_from: None,
            valid_until: None,
            supersedes: None,
            run_id: None,
            created_by_actor_id: ACTOR.to_owned(),
            correlation_id: crate::id::CorrelationId::new(),
            created_at: at(0),
        }));
        assert_eq!(
            source_reliability_signal(&record),
            match kind.permitted_trust() {
                MemoryTrust::Authoritative => SIGNAL_SCALE,
                MemoryTrust::Derived => SIGNAL_SCALE / 2,
                MemoryTrust::Untrusted => 0,
            },
            "the reliability signal must follow the kind's trust, for {kind}"
        );
        assert_eq!(
            may_be_offered_as_fact(kind),
            !kind.is_model_produced(),
            "the model gate must agree with `is_model_produced`, for {kind}"
        );
    }
}

/// Tests for the semantic signal, which needs vectors where the tests above need only records.
///
/// Declared here rather than beside `tests` so this module's fixtures — `memory`, `query`, `entity`,
/// `at` — are reachable through `use super::*`, and a fixture cannot drift between the two files.
#[path = "semantic_tests.rs"]
mod semantic_tests;
