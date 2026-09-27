//! Tests for the candidate pipeline.
//!
//! # Why these tests are the adversarial cases
//!
//! `memory-and-context.md` states the admission rules as things that must *not* happen: an inference must
//! never persist as fact, a provider must not speak for a preference, a contradictory claim must not be
//! overwritten silently, a deleted claim must not return. Each test below is the attempt the rule
//! describes rather than the happy path, because a happy-path test passes against an implementation that
//! does the wrong thing in every case the rule is about.
//!
//! The `P4-002` experience is why: two test-fixture defects there surfaced only as confusing failures,
//! which is the cost of a fixture that cannot satisfy its own precondition.

use super::*;
use crate::id::{MemoryId, WorkspaceId};
use crate::memory::{InvalidMemory, MemoryRecord, MemoryRecordParts, MemorySource};
use crate::sensitivity::Sensitivity;
use crate::timestamp::UtcTimestamp;

const ACTOR: &str = "local-user";

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

/// Extracts the values to store from an admission, panicking with the whole admission when there are none.
///
/// A named helper rather than `expect` at each site: the panic message names the *admission*, so a failing
/// assertion says which variant it got instead of "called `Option::unwrap()` on a `None`".
fn writes(admission: &MemoryAdmission) -> &MemoryToStore {
    match admission.to_store() {
        Some(to_store) => to_store,
        None => panic!("this admission writes nothing: {admission:?}"),
    }
}

fn workspace() -> WorkspaceId {
    must("0198f000-0000-7000-8000-000000000001".parse())
}

fn at(minute: i128) -> UtcTimestamp {
    // A whole-second base with a non-zero fraction, per `P4-002`'s note: a fraction of exactly zero is
    // omitted by the Rfc3339 rendering, which makes the text width unstable across rows.
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn entity() -> crate::id::EntityId {
    crate::id::EntityId::new()
}

fn user_source() -> MemorySource {
    must(MemorySource::of_kind(
        MemorySourceKind::UserStatement,
        "session:0198f000-0000-7000-8000-000000000003",
    ))
}

/// A candidate with every field overridable at the call site, starting from a valid value so a test states
/// only the field it is about.
fn candidate(
    memory_type: MemoryType,
    source_kind: MemorySourceKind,
    content: &str,
) -> MemoryCandidate {
    MemoryCandidate {
        workspace_id: workspace(),
        content: content.to_owned(),
        classification: CandidateClassification {
            memory_type,
            source_kind,
        },
        proposed_sensitivity: Sensitivity::Public,
        proposed_confidence: MemoryConfidence::Confirmed,
        importance: 1,
        proposed_entities: vec![EntityRef::confirmed(entity())],
        structured_claim: None,
        source_locator: "session:0198f000-0000-7000-8000-000000000003".to_owned(),
        source_excerpt_hash: None,
        run_id: None,
        supersedes: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
    }
}

/// A context where the store knows nothing, which is the shape most tests start from.
fn empty_context(entities: &[EntityRef]) -> CandidateContext<'_> {
    CandidateContext {
        resolved_entities: entities,
        existing: None,
        superseded_by: None,
        tombstoned: false,
    }
}

/// A stored memory built through the real constructor, so a fixture cannot hold a value the domain refuses.
fn stored(content: &str, memory_type: MemoryType, entity_ref: EntityRef) -> MemoryRecord {
    must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type,
        content: content.to_owned(),
        structured_claim: None,
        source: user_source(),
        confidence: MemoryConfidence::Confirmed,
        importance: 1,
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
// Invariant: an inferred preference never appears as confirmed fact
// ------------------------------------------------------------------------------------------------

/// **A model inference is admitted as a proposal, at `Unverified`, whatever it claimed.**
///
/// The acceptance invariant, and the one case where **lowering the confidence is not enough**: a model
/// inference is stored as a proposal even at `Unverified`, because the claim has no source that can back
/// it at all. Both halves are asserted, because an implementation that only lowered the confidence would
/// pass a test asserting the level and store the claim as current truth.
#[test]
fn a_model_inference_is_never_admitted_as_fact() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let mut proposal = candidate(
        MemoryType::Preference,
        MemorySourceKind::ModelInference,
        "Prefers dark roast coffee",
    );
    proposal.proposed_confidence = MemoryConfidence::Confirmed;

    let admission = must(proposal.admit(&empty_context(&resolved)));
    assert!(
        admission.is_proposal(),
        "a model inference must be a proposal, got {admission:?}"
    );
    let to_store = writes(&admission);
    assert_eq!(
        to_store.confidence,
        MemoryConfidence::Unverified,
        "a model inference may carry no confidence at all"
    );
    assert_eq!(
        to_store.source_trust,
        MemoryTrust::Derived,
        "the trust is derived from the kind, not proposed"
    );
}

/// **An overconfident statement from a weaker source is lowered rather than discarded.**
///
/// The other half of the same rule, and it distinguishes "the source cannot support this" from "the claim
/// is unusable". A document is a record of its own text, so it can support `Likely` — a claim that its
/// *meaning* was read. Refusing would discard a claim the user would want, so the stage lowers.
#[test]
fn an_overconfident_claim_is_lowered_rather_than_refused() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let mut from_document = candidate(
        MemoryType::Semantic,
        MemorySourceKind::Document,
        "The manual states the warranty is two years",
    );
    from_document.proposed_confidence = MemoryConfidence::Confirmed;

    let admission = must(from_document.admit(&empty_context(&resolved)));
    assert!(
        matches!(admission, MemoryAdmission::New(_)),
        "a document claim is a fact at its own ceiling, got {admission:?}"
    );
    assert_eq!(
        writes(&admission).confidence,
        MemoryConfidence::Likely,
        "a document cannot support more than a reading of its own text"
    );

    // The ceiling is reported so a caller is not merely handed a different value than it proposed.
    assert_eq!(
        confidence_ceiling(MemorySourceKind::Document),
        MemoryConfidence::Likely
    );
}

/// **A confidence below the ceiling is never raised.**
///
/// The direction that must not be corrected. A caller that proposed `Uncertain` is claiming to be unsure,
/// and raising it to `Confirmed` because the source *could* support more would make the pipeline assert
/// something the extractor declined to. Falsifying this needs only a max instead of a conditional.
#[test]
fn an_underconfident_claim_is_left_alone() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let mut cautious = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "The user mentioned a city",
    );
    cautious.proposed_confidence = MemoryConfidence::Uncertain;

    let admission = must(cautious.admit(&empty_context(&resolved)));
    assert_eq!(
        writes(&admission).confidence,
        MemoryConfidence::Uncertain,
        "a caller claiming uncertainty must not be overruled"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: trust does not transfer between claim kinds
// ------------------------------------------------------------------------------------------------

/// **A provider record cannot back a preference, however authoritative the provider is.**
///
/// The document's own example: a provider is "authoritative for an event timestamp but not for a person's
/// preference". Checked before the confidence rule so the reported cause is the real one — a lowered
/// confidence would name a rule that is not what refused it.
#[test]
fn a_provider_record_cannot_back_a_preference() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let from_provider = candidate(
        MemoryType::Preference,
        MemorySourceKind::ProviderRecord,
        "Prefers decaf",
    );
    assert_eq!(
        from_provider.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::ProviderCannotPreference)
    );

    // The control: the same source backing a claim it *can* speak to. Without it, a pipeline that refused
    // every provider record would pass the assertion above.
    let event = candidate(
        MemoryType::Episodic,
        MemorySourceKind::ProviderRecord,
        "Met on a Tuesday",
    );
    assert!(
        matches!(
            must(event.admit(&empty_context(&resolved))),
            MemoryAdmission::New(_)
        ),
        "a provider is authoritative about what it recorded"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: sensitivity is a ceiling on disclosure, never a label from the model
// ------------------------------------------------------------------------------------------------

/// **A credential in the text raises the level above anything the extractor proposed.**
///
/// The case the memory *type* says nothing about: "my API key is sk-…" is an ordinary semantic claim whose
/// content is the secret. The assertion is on the stored level because the document's rule is
/// destination-facing — exclusion from a remote model is decided from the stored level, so a level below
/// the floor is a disclosure no later check recovers.
#[test]
fn a_credential_in_the_text_raises_the_level() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let mut leaky = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "My API key is sk-abcdef1234567890",
    );
    leaky.proposed_sensitivity = Sensitivity::Public;

    let admission = must(leaky.admit(&empty_context(&resolved)));
    assert_eq!(
        writes(&admission).sensitivity,
        Sensitivity::Restricted,
        "content carrying a credential must not be classified public"
    );

    // And a health-shaped claim, which the same check must catch for the same reason.
    let mut health = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "Taking medication for the diagnosis",
    );
    health.proposed_sensitivity = Sensitivity::Internal;
    let health_admission = must(health.admit(&empty_context(&resolved)));
    assert_eq!(
        writes(&health_admission).sensitivity,
        Sensitivity::Restricted
    );
}

/// **A relationship claim is confidential even when its text is innocuous.**
///
/// The *type* floor, which the content check cannot see: "Alice is my sister" carries no keyword, and the
/// claim is about a person. This is why the two floors are separate functions — one is about the claim's
/// class and the other about its text.
#[test]
fn a_relationship_claim_is_confidential_and_never_a_fact() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let relationship = candidate(
        MemoryType::Relationship,
        MemorySourceKind::UserStatement,
        "Alice is my sister",
    );

    let admission = must(relationship.admit(&empty_context(&resolved)));
    assert!(
        admission.is_proposal(),
        "a relationship claim requires confirmation, got {admission:?}"
    );
    assert_eq!(
        writes(&admission).sensitivity,
        Sensitivity::Confidential,
        "a claim about a person is confidential by its type"
    );
}

/// **An over-classification by the extractor is kept.**
///
/// The direction that must not be corrected downward: raising the level withholds content from a remote
/// model, which is a cost, while lowering it is a disclosure. A pipeline that normalized every candidate
/// to the floor would silently publish what a caller had protected.
#[test]
fn a_higher_proposed_classification_is_kept() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let mut careful = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "Lives in Rotterdam",
    );
    careful.proposed_sensitivity = Sensitivity::Confidential;

    let careful_admission = must(careful.admit(&empty_context(&resolved)));
    assert_eq!(
        writes(&careful_admission).sensitivity,
        Sensitivity::Confidential,
        "a caller raising the level must not be overruled"
    );

    // And the content check returns `Public` for unrecognized text, so it cannot silently raise everything
    // to the default — which would make the floor test above pass for the wrong reason.
    assert_eq!(
        content_sensitivity_floor("Lives in Rotterdam"),
        Sensitivity::Public
    );
    assert_eq!(
        type_sensitivity_floor(MemoryType::Semantic),
        Sensitivity::Internal
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: contradictory memory is never overwritten silently
// ------------------------------------------------------------------------------------------------

/// **A repeated claim is a duplicate: the store is told to reinforce, not to write a second row.**
#[test]
fn a_repeated_claim_is_a_duplicate() {
    let subject = entity();
    let entity_ref = EntityRef::confirmed(subject);
    let resolved = [entity_ref.clone()];
    let existing = stored(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        entity_ref,
    );

    let repeat = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    let context = CandidateContext {
        resolved_entities: &resolved,
        existing: Some(&existing),
        superseded_by: None,
        tombstoned: false,
    };
    assert_eq!(
        must(repeat.admit(&context)),
        MemoryAdmission::Duplicate {
            existing_memory_id: existing.id()
        }
    );

    // The action the caller must take is derivable from the admission, which is what makes the value
    // usable without consulting a separate reason field.
    assert!(
        !must(repeat.admit(&context)).writes(),
        "a duplicate must not write a second memory"
    );
}

/// **A declared correction supersedes, and a duplicate is what an un-declared equal claim is.**
///
/// Why the correction is declared rather than inferred, which the fixture demonstrates directly: two
/// claims sharing a search key **have the same words** — the key is the sorted, case-folded word set — so
/// a different claim has a different key and no `existing` row to compare against. Inferring from "same
/// entity, different words" was the next attempt and it retires a memory for every unrelated fact about a
/// person, since "Alice is my sister" and "Alice lives in Rotterdam" share an entity.
///
/// So the same candidate content produces a `Duplicate` when nothing is declared and a `Correction` when
/// the replacement is declared, and the two assertions differ only by that field.
#[test]
fn a_correction_is_declared_and_a_duplicate_is_what_remains() {
    let subject = entity();
    let entity_ref = EntityRef::confirmed(subject);
    let resolved = [entity_ref.clone()];
    let existing = stored(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        entity_ref,
    );

    let context = CandidateContext {
        resolved_entities: &resolved,
        existing: Some(&existing),
        superseded_by: None,
        tombstoned: false,
    };

    // The key must actually match the stored memory's, or the duplicate branch is unreachable and the test
    // would be about two unrelated claims.
    let undeclared = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    assert_eq!(
        memory_key(&undeclared, &resolved),
        memory_key_of(&existing),
        "the fixture must share a key, or the branch under test is unreachable"
    );
    assert_eq!(
        must(undeclared.admit(&context)),
        MemoryAdmission::Duplicate {
            existing_memory_id: existing.id()
        },
        "a matching claim with no declared replacement is a duplicate"
    );

    // Declared: the same content, with the replacement named.
    let mut correction = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserCorrection,
        "Prefers dark roast coffee",
    );
    correction.supersedes = Some(existing.id());
    match must(correction.admit(&context)) {
        MemoryAdmission::Correction { supersedes, .. } => {
            assert_eq!(
                supersedes,
                existing.id(),
                "the correction must name the memory it replaces"
            );
        }
        other => panic!("a declared correction must supersede, got {other:?}"),
    }
}

/// **A claim the store already replaced writes nothing, and does not reinforce the replacement.**
///
/// The third comparison outcome. Collapsing it into `Duplicate` would count a retrieval against a memory
/// whose claim the candidate did not repeat, which corrupts the reinforcement signal `P4-004` ranks by.
#[test]
fn an_already_superseded_claim_writes_nothing() {
    let subject = entity();
    let entity_ref = EntityRef::confirmed(subject);
    let resolved = [entity_ref];
    let current = MemoryId::new();

    let stale = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    let context = CandidateContext {
        resolved_entities: &resolved,
        existing: None,
        superseded_by: Some(current),
        tombstoned: false,
    };
    let admission = must(stale.admit(&context));
    assert_eq!(
        admission,
        MemoryAdmission::AlreadySuperseded {
            current_memory_id: current
        }
    );
    assert!(!admission.writes());
    assert_eq!(
        admission.reason(),
        Some(CandidateRefusal::AlreadySuperseded {
            current_memory_id: current
        })
    );
}

/// **A correction outranks a proposal.**
///
/// The ordering that decides whether a corrected *relationship* claim becomes a proposal or a correction.
/// Both are legal admissions for such a candidate and only one is right: a proposal leaves the obsolete
/// claim as current truth, which the document's "stop retrieving obsolete claims as current truth"
/// forbids. The fixture is deliberately a relationship — the type that always proposes — so the test fails
/// if the proposal branch is reached first.
#[test]
fn a_correction_of_a_relationship_claim_is_still_a_correction() {
    let subject = entity();
    let entity_ref = EntityRef::confirmed(subject);
    let resolved = [entity_ref.clone()];
    let existing = stored("Alice is my sister", MemoryType::Relationship, entity_ref);

    let mut correction = candidate(
        MemoryType::Relationship,
        MemorySourceKind::UserCorrection,
        "Alice is my sister",
    );
    correction.supersedes = Some(existing.id());
    let context = CandidateContext {
        resolved_entities: &resolved,
        existing: Some(&existing),
        superseded_by: None,
        tombstoned: false,
    };
    let admission = must(correction.admit(&context));
    assert!(
        matches!(admission, MemoryAdmission::Correction { .. }),
        "a correction must not become a proposal, got {admission:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: deletion removes the claim and blocks resurrection
// ------------------------------------------------------------------------------------------------

/// **A deleted claim cannot be re-ingested, and the tombstone is checked before the duplicate.**
///
/// Both halves matter and the second is an ordering property. A deleted memory's row retains no search key
/// — `P4-002` clears it — so it can never appear as `existing`; a pipeline that checked the duplicate first
/// would see "nothing there" and write a fresh row for a claim the user removed. The fixture is therefore
/// **both** tombstoned and matching an existing memory, which is the only way the ordering is observable.
#[test]
fn a_deleted_claim_cannot_return() {
    let subject = entity();
    let entity_ref = EntityRef::confirmed(subject);
    let resolved = [entity_ref.clone()];
    let existing = stored(
        "Prefers dark roast coffee",
        MemoryType::Preference,
        entity_ref,
    );

    let resurrect = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    let context = CandidateContext {
        resolved_entities: &resolved,
        existing: Some(&existing),
        superseded_by: None,
        tombstoned: true,
    };
    assert_eq!(
        resurrect.admit(&context),
        Err(CandidateRefusal::Tombstoned),
        "the tombstone must be checked before the duplicate, or a deleted claim returns"
    );
}

// ------------------------------------------------------------------------------------------------
// Invariant: a claim about nothing is not a memory
// ------------------------------------------------------------------------------------------------

/// **An unresolved entity is refused, and the extractor's proposal is not a fallback.**
///
/// Retrieval is by entity, so a memory with an unresolved subject is a claim whose retrieval key is a
/// guess. The candidate deliberately *has* a proposed entity, so the test fails if proposals are used as a
/// fallback — which is the tempting shape, since the proposals are right there.
#[test]
fn an_unresolved_entity_is_refused_and_proposals_are_not_a_fallback() {
    let claim = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    assert!(
        !claim.proposed_entities.is_empty(),
        "the fixture must have a proposal, or the fallback would be untestable"
    );
    assert_eq!(
        claim.admit(&empty_context(&[])),
        Err(CandidateRefusal::EntityUnresolved)
    );
}

/// **The key is derived from the resolved entities, not from the proposals.**
///
/// The reason resolution happens before the key: a key built from a proposal names an entity the store does
/// not have, so the candidate would compare against the wrong row and either write a duplicate or miss a
/// correction. The fixture's proposal and resolution are **different** entities, so the two derivations
/// give different keys and only one can be asserted.
#[test]
fn the_key_is_derived_from_resolved_entities() {
    let claimed = entity();
    let resolved_id = entity();
    assert_ne!(claimed, resolved_id);

    let mut claim = candidate(
        MemoryType::Preference,
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
    );
    claim.proposed_entities = vec![EntityRef::confirmed(claimed)];
    let resolved = [EntityRef::confirmed(resolved_id)];

    let admission = must(claim.admit(&empty_context(&resolved)));
    let expected = memory_key(&claim, &resolved);
    assert_eq!(writes(&admission).search_key, expected);
    assert_eq!(writes(&admission).entities, resolved);
}

// ------------------------------------------------------------------------------------------------
// Shape rules
// ------------------------------------------------------------------------------------------------

/// **Empty content, an oversized content, and an unsourced candidate are each refused by name.**
///
/// Three shape rules with three different causes. Collapsing them into one refusal would send an operator
/// debugging the wrong field — the same reasoning `P4-001` records for its own error variants.
#[test]
fn the_shape_rules_are_distinct() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];

    let empty = candidate(MemoryType::Semantic, MemorySourceKind::UserStatement, "   ");
    assert_eq!(
        empty.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::Content)
    );

    let oversized = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        &"a".repeat(crate::memory::MAX_MEMORY_CONTENT_CHARS + 1),
    );
    assert_eq!(
        oversized.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::Content),
        "the candidate bound must be the memory's, or content it could hold is refused"
    );

    let mut unsourced = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "A claim",
    );
    unsourced.source_locator = "  ".to_owned();
    assert_eq!(
        unsourced.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::Unsupported),
        "a claim with no source is not a memory"
    );

    let mut crowded = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        "A claim",
    );
    crowded.proposed_entities = (0..=MAX_CANDIDATE_ENTITIES)
        .map(|_| EntityRef::confirmed(entity()))
        .collect();
    assert_eq!(
        crowded.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::TooManyEntities)
    );
}

/// **The content bound counts characters, not bytes, and the key's own bound is a different rule.**
///
/// Two bounds that a single word would conflate. The content bound is about length; the search key's rule is
/// about the key's *shape*, because `P4-002` bounds each key word at
/// [`crate::memory::MAX_SEARCH_KEY_WORD_CHARS`]. So a 4096-character **single word** is within the content
/// bound and still unkeyable — which is why the two refusals are separate values, and why the fixture for
/// the content bound is many short words rather than one long one. Reporting "content" for both would send
/// an operator shortening text that already fits.
#[test]
fn the_content_bound_counts_characters_and_the_key_bound_is_separate() {
    assert_eq!(
        MAX_CANDIDATE_CONTENT_CHARS,
        crate::memory::MAX_MEMORY_CONTENT_CHARS
    );
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];

    // Exactly at the bound, as ordinary prose: admitted, and the content survives to the value to store.
    let prose = "coffee ".repeat(MAX_CANDIDATE_CONTENT_CHARS / 7);
    let at_bound = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        &prose,
    );
    let stored = must(at_bound.admit(&empty_context(&resolved)));
    assert_eq!(
        writes(&stored).content.chars().count(),
        prose.trim().chars().count()
    );

    // Multi-byte characters, where a byte count and a character count disagree: 4096 'é' is 8192 bytes, so a
    // byte-counting bound would refuse content the memory would accept.
    let multibyte = "é ".repeat(crate::memory::MAX_MEMORY_CONTENT_CHARS / 2);
    let unicode = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        &multibyte,
    );
    assert!(
        unicode.admit(&empty_context(&resolved)).is_ok(),
        "the bound is characters, so 2048 two-byte characters must be admitted"
    );

    // And the key's own bound, reported as its own refusal.
    let one_long_word = candidate(
        MemoryType::Semantic,
        MemorySourceKind::UserStatement,
        &"a".repeat(crate::memory::MAX_SEARCH_KEY_WORD_CHARS + 1),
    );
    assert_eq!(
        one_long_word.admit(&empty_context(&resolved)),
        Err(CandidateRefusal::Unkeyable),
        "an unkeyable word is not an over-long content"
    );
}

// ------------------------------------------------------------------------------------------------
// Normalization
// ------------------------------------------------------------------------------------------------

/// **The candidate's comparison normalizes exactly as the search key does.**
///
/// The property that keeps "duplicate" and "same key" the same relation. If they disagreed, a candidate
/// differing only in spacing would be reported as a *correction* — which supersedes a memory, retiring a
/// claim for a formatting difference. The control is a genuinely different claim, because a comparison
/// that returned `true` for everything would pass the first half.
#[test]
fn the_comparison_matches_the_search_key_normalization() {
    assert!(normalized_equal(
        "Prefers dark roast coffee",
        "prefers   dark roast COFFEE"
    ));
    assert!(normalized_equal("roast dark prefers", "Prefers dark roast"));
    assert!(
        !normalized_equal("Prefers dark roast coffee", "Prefers light roast coffee"),
        "a different claim must not compare equal"
    );
    assert!(
        !normalized_equal("Prefers dark roast", "Prefers dark roast coffee"),
        "an extra word must not compare equal"
    );

    // The same three inputs through the key itself, so the two normalizations cannot drift: two contents
    // that compare equal must produce one key.
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    let key_of = |content: &str| {
        must(MemorySearchKey::new(
            MemoryType::Preference,
            &resolved,
            content,
        ))
        .as_str()
        .to_owned()
    };
    assert_eq!(
        key_of("Prefers dark roast coffee"),
        key_of("prefers   dark roast COFFEE")
    );
}

// ------------------------------------------------------------------------------------------------
// The vocabulary's own consistency
// ------------------------------------------------------------------------------------------------

/// **Every source kind's ceiling and proposal rule are stated, and the table is the one asserted.**
///
/// A row in a mapping table is a claim until a test pins it. The two properties are asserted per kind
/// because they are the two ways a kind can be mishandled: a ceiling that lets an inference be a fact, and
/// a proposal rule that lets one be current truth.
#[test]
fn every_source_kind_has_a_ceiling_and_a_proposal_rule() {
    for kind in MemorySourceKind::all() {
        let ceiling = confidence_ceiling(kind);
        let classification = CandidateClassification {
            memory_type: MemoryType::Semantic,
            source_kind: kind,
        };
        assert_eq!(classification.confidence_ceiling(), ceiling);
        assert_eq!(
            classification.requires_proposal(),
            kind.is_model_produced(),
            "only a model inference forces a proposal on its own, for {kind}"
        );
        assert_eq!(
            ceiling == MemoryConfidence::Unverified,
            kind.is_model_produced(),
            "the `Unverified` ceiling and the model-inferred kind are the same set, for {kind}"
        );
    }

    // The refusal vocabulary round-trips its own names, so a log line names a variant that exists.
    assert_eq!(
        CandidateRefusal::Tombstoned.as_str(),
        "tombstoned",
        "a rename would silently change a stored or logged reason"
    );
    assert!(
        !CandidateRefusal::ConfidenceLowered {
            proposed: MemoryConfidence::Confirmed,
            ceiling: MemoryConfidence::Likely,
        }
        .forbids_writing()
    );
    assert!(CandidateRefusal::Content.forbids_writing());
}

/// **A candidate's trust is derived from its source kind, and the classification cannot disagree.**
///
/// `P4-001` refuses a source whose trust disagrees with its kind, and the pipeline builds the trust from
/// the kind rather than accepting one — so the refusal is unreachable from here. The test asserts the
/// derivation for every kind rather than the absence of the failure, because "unreachable" is what the
/// first half of an invariant looks like and the second half is the value.
#[test]
fn the_source_trust_is_derived_from_the_kind() {
    let subject = entity();
    let resolved = [EntityRef::confirmed(subject)];
    for kind in MemorySourceKind::all() {
        let mut claim = candidate(MemoryType::Episodic, kind, "A claim about an event");
        claim.proposed_confidence = MemoryConfidence::Unverified;
        let admission = must(claim.admit(&empty_context(&resolved)));
        assert_eq!(
            writes(&admission).source_trust,
            kind.permitted_trust(),
            "the trust must come from the kind, for {kind}"
        );
    }

    // And the domain agrees: a source built from a kind with the wrong trust is refused by its constructor,
    // so the pipeline's derivation is the only shape that reaches storage.
    let mismatched = MemorySource::new(
        MemorySourceKind::ExternalContent,
        "https://example.invalid/page",
        MemoryTrust::Authoritative,
        None,
    );
    assert!(matches!(mismatched, Err(InvalidMemory::Source)));
}

/// The search key a candidate would carry, for a fixture that must share one with a stored memory.
fn memory_key(candidate: &MemoryCandidate, entities: &[EntityRef]) -> String {
    must(MemorySearchKey::new(
        candidate.classification.memory_type,
        entities,
        candidate.content.trim(),
    ))
    .as_str()
    .to_owned()
}

/// The search key of a stored memory, derived from its own fields.
fn memory_key_of(record: &MemoryRecord) -> String {
    must(MemorySearchKey::new(
        record.memory_type(),
        record.entities(),
        record.content(),
    ))
    .as_str()
    .to_owned()
}
