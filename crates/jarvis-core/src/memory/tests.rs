//! Tests for the memory vocabulary and its invariants.
//!
//! Each test names the rule it holds, because a test whose name is "it works" cannot tell a later reader
//! which behaviour they are about to break.

use super::*;
use crate::id::MemoryId;
use crate::id::WorkspaceId;

const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000001";
const ACTOR: &str = "0198f000-0000-7000-8000-0000000000a1";
const RUN: &str = "0198f000-0000-7000-8000-0000000000c3";

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

/// A timestamp `minute` minutes from a **fixed, non-zero-fraction** base instant.
///
/// The base carries a sub-second part deliberately: `UtcTimestamp`'s `Rfc3339` rendering omits the fraction
/// when it is zero, and the omitted form sorts after the fractional form within one second. A base of
/// exactly `.000000000` would put this helper one tick away from mixing widths.
fn at(minute: i128) -> UtcTimestamp {
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn workspace() -> WorkspaceId {
    must(WORKSPACE.parse())
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

/// A preference the user stated, which is the fixture most invariants are about.
fn preference() -> MemoryRecord {
    must(MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Preference,
        content: "Prefers dark roast coffee".to_owned(),
        structured_claim: Some(must(StructuredClaim::new(
            "user",
            "prefers",
            "dark roast coffee",
        ))),
        source: user_source(),
        confidence: MemoryConfidence::Confirmed,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(entity())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: Some(must(RUN.parse())),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }))
}

/// Parts for a memory, with every field overridable at the call site.
///
/// A struct-update helper rather than fifteen parameters, and it starts from a **valid** value so a test
/// states only the field it is about â€” which keeps the failing assertion pointing at the rule under test.
fn parts(memory_type: MemoryType, source: MemorySource) -> MemoryRecordParts {
    MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type,
        content: "A claim".to_owned(),
        structured_claim: None,
        source,
        confidence: MemoryConfidence::Confirmed,
        importance: 1,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(entity())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    }
}

/// **Every vocabulary value round-trips through its storage name.**
///
/// The property a schema `CHECK` and a decoded row both rest on. A value whose `as_str` and `FromStr`
/// disagree would store under one name and be refused on read, which reads as corruption rather than a bug.
#[test]
fn every_vocabulary_value_round_trips() {
    for memory_type in MemoryType::all() {
        assert_eq!(
            memory_type.as_str().parse::<MemoryType>(),
            Ok(memory_type),
            "{memory_type}"
        );
    }
    for confidence in MemoryConfidence::all() {
        assert_eq!(
            confidence.as_str().parse::<MemoryConfidence>(),
            Ok(confidence)
        );
    }
    for status in MemoryStatus::all() {
        assert_eq!(status.as_str().parse::<MemoryStatus>(), Ok(status));
    }
    for kind in MemorySourceKind::all() {
        assert_eq!(kind.as_str().parse::<MemorySourceKind>(), Ok(kind));
    }
    for trust in [
        MemoryTrust::Untrusted,
        MemoryTrust::Derived,
        MemoryTrust::Authoritative,
    ] {
        assert_eq!(trust.as_str().parse::<MemoryTrust>(), Ok(trust));
    }
    for matched in [
        EntityMatch::Confirmed,
        EntityMatch::ProviderId,
        EntityMatch::ExactIdentifier,
        EntityMatch::Probabilistic,
    ] {
        assert_eq!(matched.as_str().parse::<EntityMatch>(), Ok(matched));
    }
    // An unknown name is refused rather than defaulted, so a row from a newer build is reported.
    assert_eq!(
        "telepathic".parse::<MemoryType>(),
        Err(InvalidMemory::UnknownType)
    );
    assert_eq!(
        "certain".parse::<MemoryConfidence>(),
        Err(InvalidMemory::UnknownConfidence)
    );
    assert_eq!(
        "dormant".parse::<MemoryStatus>(),
        Err(InvalidMemory::UnknownStatus)
    );
}

/// **A model inference cannot be recorded as anything but unverified.**
///
/// The acceptance invariant "an inferred preference never appears as confirmed fact", made unbuildable
/// rather than merely discouraged. The falsification is the point: without the constructor check, a caller
/// â€” or a model-produced candidate â€” could record `Confirmed` for something nothing confirmed.
#[test]
fn a_model_inference_cannot_claim_confidence() {
    let source = must(MemorySource::of_kind(
        MemorySourceKind::ModelInference,
        "run:0198f000-0000-7000-8000-0000000000c3",
    ));

    // At `Unverified` it is a proposal, which is what the architecture permits.
    let mut unverified = parts(MemoryType::Semantic, source.clone());
    unverified.confidence = MemoryConfidence::Unverified;
    assert!(MemoryRecord::new(unverified).is_ok());

    // At every higher level it is refused, and the check is the constructor's rather than a caller's.
    for confidence in [
        MemoryConfidence::Uncertain,
        MemoryConfidence::Likely,
        MemoryConfidence::Confirmed,
    ] {
        let mut candidate = parts(MemoryType::Semantic, source.clone());
        candidate.confidence = confidence;
        assert_eq!(
            MemoryRecord::new(candidate),
            Err(InvalidMemory::Source),
            "a model inference must not be recordable at {confidence}"
        );
    }
}

/// **A provider record cannot back a preference claim.**
///
/// `memory-and-context.md`: "A provider may be authoritative for an event timestamp but not for a person's
/// preference." The trust class does not transfer between claim kinds, and the refusal lives at the
/// constructor so a reader does not have to apply the rule.
#[test]
fn a_provider_record_cannot_back_a_preference() {
    let provider = must(MemorySource::of_kind(
        MemorySourceKind::ProviderRecord,
        "provider:message/abc123",
    ));

    let mut as_preference = parts(MemoryType::Preference, provider.clone());
    as_preference.confidence = MemoryConfidence::Confirmed;
    assert_eq!(
        MemoryRecord::new(as_preference),
        Err(InvalidMemory::Source),
        "a provider record is not authoritative about a preference"
    );

    // The same source about an event it recorded is accepted, so the refusal is about the claim kind
    // rather than about the source.
    let mut as_episodic = parts(MemoryType::Episodic, provider);
    as_episodic.content = "A meeting happened at 10:00".to_owned();
    assert!(
        MemoryRecord::new(as_episodic).is_ok(),
        "a provider record is authoritative about what it recorded"
    );
}

/// **A source cannot claim a trust class its kind does not carry.**
///
/// The injection boundary: external content labelled `Authoritative` is refused at construction, so the
/// class a reader sees is one the kind could actually have.
#[test]
fn a_source_cannot_claim_a_trust_it_does_not_have() {
    assert_eq!(
        MemorySource::new(
            MemorySourceKind::ExternalContent,
            "https://example.invalid/poem",
            MemoryTrust::Authoritative,
            None,
        ),
        Err(InvalidMemory::Source),
        "external content must never be authoritative"
    );
    assert_eq!(
        MemorySource::new(
            MemorySourceKind::UserStatement,
            "session:abc",
            MemoryTrust::Untrusted,
            None,
        ),
        Err(InvalidMemory::Source),
        "a user statement must not be recorded as untrusted either, or the record denies its own origin"
    );
    assert!(
        MemorySource::new(
            MemorySourceKind::ExternalContent,
            "https://example.invalid/poem",
            MemoryTrust::Untrusted,
            None,
        )
        .is_ok()
    );
}

/// **A retrieved memory in a superseded or expired state is not current truth.**
///
/// The invariant "correcting it removes the old claim from current retrieval while retaining an allowed
/// audit trail": the trail is the `Archived` row, and it must not answer as though it were current.
#[test]
fn only_an_active_memory_is_current_truth() {
    let memory = preference();
    assert!(memory.effective_status_at(at(0)).is_current_truth());

    // Superseded: archived, and no longer current, but still present for audit.
    let superseded = must(memory.replace_with(MemoryId::new(), at(1)));
    assert_eq!(superseded.status(), MemoryStatus::Archived);
    assert_eq!(
        superseded.effective_status_at(at(2)),
        EffectiveMemoryStatus::Superseded
    );
    assert!(!superseded.effective_status_at(at(2)).is_current_truth());
    assert_eq!(
        superseded.content(),
        memory.content(),
        "the superseded claim keeps its text: the audit trail is the point"
    );
}

/// **An expired memory stops being current truth, and the state is derived rather than stored.**
#[test]
fn a_lapsed_memory_is_reported_as_expired() {
    let mut candidate = parts(MemoryType::Semantic, user_source());
    candidate.valid_until = Some(at(10));
    let memory = must(MemoryRecord::new(candidate));

    assert_eq!(
        memory.effective_status_at(at(9)),
        EffectiveMemoryStatus::Active
    );
    assert_eq!(
        memory.effective_status_at(at(10)),
        EffectiveMemoryStatus::Expired,
        "the boundary itself is outside the window"
    );
    assert_eq!(
        memory.status(),
        MemoryStatus::Active,
        "the stored status is unchanged"
    );

    // The single presentation predicate answers both questions, so a caller cannot apply one and forget
    // the other: a `Confirmed` claim that has lapsed is not stateable as fact.
    assert!(memory.is_stateable_as_fact_at(at(9)));
    assert!(!memory.is_stateable_as_fact_at(at(10)));
}

/// **A claim supported only `Likely` is current but is not stated as fact.**
///
/// The other half of the same predicate, which a caller checking only the status would miss.
#[test]
fn a_likely_claim_is_current_but_hedged() {
    let mut candidate = parts(MemoryType::Semantic, user_source());
    candidate.confidence = MemoryConfidence::Likely;
    let memory = must(MemoryRecord::new(candidate));

    assert!(memory.effective_status_at(at(0)).is_current_truth());
    assert!(
        !memory.is_stateable_as_fact_at(at(0)),
        "a merely likely claim must not be presented as established"
    );
    assert!(!memory.confidence().is_stated_as_fact());
    assert!(MemoryConfidence::Confirmed.is_stated_as_fact());
}

/// **Deleting a memory removes its text and is terminal.**
///
/// The acceptance invariant "deleting it removes text and derived indexes". The domain clears the text so
/// a value already held in memory cannot be returned, and the second delete is refused so a caller learns
/// it is acting on a stale read.
#[test]
fn deletion_removes_the_text_and_is_terminal() {
    let memory = preference();
    let deleted = must(memory.delete(at(5)));

    assert_eq!(deleted.status(), MemoryStatus::Deleted);
    assert!(
        deleted.content().is_empty(),
        "the deleted text must not remain in the value"
    );
    assert!(deleted.structured_claim().is_none());
    assert!(deleted.superseded_by().is_none());
    assert_eq!(
        deleted.effective_status_at(at(6)),
        EffectiveMemoryStatus::Deleted
    );
    assert!(!deleted.effective_status_at(at(6)).is_current_truth());

    // Terminal: every transition is refused, and each for its own reason.
    assert_eq!(deleted.delete(at(7)), Err(InvalidMemory::AlreadyDeleted));
    assert_eq!(deleted.confirm(at(7)), Err(InvalidMemory::AlreadyDeleted));
    assert_eq!(deleted.archive(at(7)), Err(InvalidMemory::AlreadyDeleted));
    assert_eq!(
        deleted.replace_with(MemoryId::new(), at(7)),
        Err(InvalidMemory::AlreadyDeleted)
    );
}

/// **A relationship memory starts as a proposal, because the class requires confirmation.**
///
/// The document: "High-impact identity, medical, financial, authentication, and relationship inferences
/// require explicit user confirmation before becoming trusted facts." The status is derived from the type
/// at construction, so a caller cannot record one already active.
#[test]
fn a_relationship_memory_cannot_start_active() {
    let mut candidate = parts(
        MemoryType::Relationship,
        must(MemorySource::of_kind(
            MemorySourceKind::ExternalContent,
            "https://example.invalid/org",
        )),
    );
    candidate.confidence = MemoryConfidence::Unverified;
    let memory = must(MemoryRecord::new(candidate));

    assert_eq!(
        memory.status(),
        MemoryStatus::Proposed,
        "a relationship claim must not be recorded as current"
    );
    assert!(!memory.effective_status_at(at(0)).is_current_truth());

    // Confirming it is the explicit step, and it is the only way to current.
    let confirmed = must(memory.confirm(at(1)));
    assert_eq!(confirmed.status(), MemoryStatus::Active);
    assert!(confirmed.effective_status_at(at(1)).is_current_truth());
    assert_eq!(confirmed.updated_at(), at(1));
}

/// **The status transition table permits restore and refuses a silent demotion.**
#[test]
fn the_status_table_permits_restore_and_refuses_demotion() {
    let memory = preference();

    // active -> active is refused, so a repeated call is reported rather than bumping `updated_at`.
    assert_eq!(
        memory.confirm(at(1)),
        Err(InvalidMemory::IllegalStatus {
            from: "active",
            to: "active"
        })
    );
    // active -> proposed is absent: "this was wrong" is a correction, not a demotion.
    assert!(!MemoryStatus::Active.can_advance_to(MemoryStatus::Proposed));
    assert!(!MemoryStatus::Archived.can_advance_to(MemoryStatus::Proposed));

    // archived -> active is present, so the audit trail is reversible.
    let archived = must(memory.archive(at(2)));
    assert_eq!(archived.status(), MemoryStatus::Archived);
    let restored = must(archived.confirm(at(3)));
    assert_eq!(restored.status(), MemoryStatus::Active);

    // Everything may be deleted.
    for status in MemoryStatus::all() {
        assert!(status.can_advance_to(MemoryStatus::Deleted), "{status}");
    }
}

/// **A correction trail cannot be rewritten.**
///
/// `memory-and-context.md` requires the correction trail be retained, so a second, different replacement is
/// refused rather than applied â€” otherwise a chain of corrections could be overwritten to hide one.
#[test]
fn a_replacement_cannot_be_rewritten() {
    let memory = preference();
    let first = MemoryId::new();
    let second = MemoryId::new();

    let replaced = must(memory.replace_with(first, at(1)));
    assert_eq!(replaced.superseded_by(), Some(first));

    // Setting the same replacement again is a no-op rather than an error, so a retried write of one fact
    // is accepted â€” the rule `P3-016` follows for a repeated link.
    let repeated = must(replaced.replace_with(first, at(2)));
    assert_eq!(repeated.superseded_by(), Some(first));

    // A **different** replacement is refused.
    assert!(
        replaced.replace_with(second, at(2)).is_err(),
        "a correction trail must not be rewritable"
    );
    // And the memory cannot replace itself.
    assert_eq!(
        memory.replace_with(memory.id(), at(1)),
        Err(InvalidMemory::SupersedesSelf)
    );
}

/// **A memory with no source, no content, or an impossible window is refused.**
#[test]
fn an_unusable_memory_is_refused() {
    // No entity at all: "a claim about what?" has no answer, and retrieval is by entity.
    let mut entityless = parts(MemoryType::Semantic, user_source());
    entityless.entities = Vec::new();
    assert_eq!(MemoryRecord::new(entityless), Err(InvalidMemory::Entities));

    // Content that is only whitespace: a stored blank claim is a row retrieval can match and a user
    // cannot read.
    let mut blank = parts(MemoryType::Semantic, user_source());
    blank.content = "   \n\t ".to_owned();
    assert_eq!(MemoryRecord::new(blank), Err(InvalidMemory::Content));

    // An oversized content field, refused rather than truncated: a shortened claim is a different claim.
    let mut oversized = parts(MemoryType::Semantic, user_source());
    oversized.content = "x".repeat(MAX_MEMORY_CONTENT_CHARS + 1);
    assert_eq!(MemoryRecord::new(oversized), Err(InvalidMemory::Content));

    // An inverted window.
    let mut inverted = parts(MemoryType::Semantic, user_source());
    inverted.valid_from = Some(at(10));
    inverted.valid_until = Some(at(5));
    assert_eq!(MemoryRecord::new(inverted), Err(InvalidMemory::Validity));

    // A window that starts before the memory existed: a claim cannot have been true before it was
    // recorded unless the source said so, and backdating is how a later correction fails to outrank it.
    let mut backdated = parts(MemoryType::Semantic, user_source());
    backdated.valid_from = Some(at(-1));
    assert_eq!(MemoryRecord::new(backdated), Err(InvalidMemory::Validity));

    // Too many entities.
    let mut crowded = parts(MemoryType::Semantic, user_source());
    crowded.entities = (0..=MAX_MEMORY_ENTITIES)
        .map(|_| EntityRef::confirmed(EntityId::new()))
        .collect();
    assert_eq!(MemoryRecord::new(crowded), Err(InvalidMemory::Entities));

    // A self-supersession.
    let mut self_superseding = parts(MemoryType::Semantic, user_source());
    let id = self_superseding.id;
    self_superseding.supersedes = Some(id);
    assert_eq!(
        MemoryRecord::new(self_superseding),
        Err(InvalidMemory::SupersedesSelf)
    );
}

/// **A repeated entity is recorded with the weakest match basis, not the strongest.**
///
/// A reader of one memory cannot tell which mention it is looking at, so one confident mention must not
/// launder a later guess about the same entity.
#[test]
fn a_repeated_entity_keeps_the_weakest_match() {
    let shared = entity();
    let mut candidate = parts(MemoryType::Semantic, user_source());
    candidate.entities = vec![
        EntityRef::new(shared, EntityMatch::Confirmed),
        EntityRef::new(shared, EntityMatch::Probabilistic),
    ];
    let memory = must(MemoryRecord::new(candidate));

    assert_eq!(memory.entities().len(), 1, "the entity is recorded once");
    assert_eq!(
        memory.entities()[0].matched_by(),
        EntityMatch::Probabilistic,
        "the weaker basis must win, or a guess is laundered by a confident mention"
    );

    // The reverse order produces the same answer, so the result does not depend on the caller's ordering.
    let mut reversed = parts(MemoryType::Semantic, user_source());
    reversed.entities = vec![
        EntityRef::new(shared, EntityMatch::Probabilistic),
        EntityRef::new(shared, EntityMatch::Confirmed),
    ];
    assert_eq!(
        must(MemoryRecord::new(reversed)).entities()[0].matched_by(),
        EntityMatch::Probabilistic
    );
}

/// **A similarity match is not a basis for merging two entities.**
///
/// The document: "Never merge solely because embeddings are similar." A merge driven only by a
/// probabilistic match therefore cannot be expressed as a call this build accepts.
#[test]
fn a_similarity_match_may_not_merge() {
    assert!(!EntityMatch::Probabilistic.may_merge());
    assert!(EntityMatch::Confirmed.may_merge());
    assert!(EntityMatch::ProviderId.may_merge());
    assert!(EntityMatch::ExactIdentifier.may_merge());
}

/// **A memory is visible only in its own workspace.**
///
/// The acceptance invariant "client A memory cannot enter client B context", as a predicate the retrieval
/// slice will ask. An equality rather than a hierarchy, because a memory layer that invented an
/// inheritance rule would be the layer that leaked across the boundary.
#[test]
fn a_memory_is_visible_only_in_its_own_workspace() {
    let memory = preference();
    assert!(memory.is_visible_in(workspace()));
    assert!(!memory.is_visible_in(WorkspaceId::new()));
}

/// **A search key is stable across a re-ingest and distinct for a reworded claim.**
///
/// The two directions of the idempotency decision. Under-collapsing a duplicate costs a row; over-collapsing
/// a distinct claim loses information the user gave, so the normalization is case, whitespace, and order
/// only â€” no stemming, no stop words, no synonyms.
#[test]
fn a_search_key_collapses_repeats_without_collapsing_claims() {
    let subject = vec![EntityRef::confirmed(entity())];
    let first = must(MemorySearchKey::new(
        MemoryType::Preference,
        &subject,
        "Prefers dark roast coffee",
    ));
    let repeated = must(MemorySearchKey::new(
        MemoryType::Preference,
        &subject,
        "  prefers   DARK roast coffee  ",
    ));
    let reordered = must(MemorySearchKey::new(
        MemoryType::Preference,
        &subject,
        "coffee roast dark prefers",
    ));
    assert_eq!(first, repeated, "case and whitespace are not the claim");
    assert_eq!(first, reordered, "word order is not the claim");

    // A different claim is a different key, which is what keeps a correction from being dropped as a
    // duplicate.
    let different = must(MemorySearchKey::new(
        MemoryType::Preference,
        &subject,
        "Prefers light roast coffee",
    ));
    assert_ne!(
        first, different,
        "a reworded claim must not collapse into the one it corrects"
    );

    // A different type is a different key even with identical text, because the type carries retention.
    let other_type = must(MemorySearchKey::new(
        MemoryType::Episodic,
        &subject,
        "Prefers dark roast coffee",
    ));
    assert_ne!(first, other_type);

    // Content with no usable word is refused rather than producing an empty key.
    assert_eq!(
        MemorySearchKey::new(MemoryType::Semantic, &subject, "!!! ..."),
        Err(InvalidMemory::SearchKey)
    );
}

/// **A structured claim renders one canonical key and refuses an unusable part.**
#[test]
fn a_structured_claim_is_bounded_and_canonical() {
    let claim = must(StructuredClaim::new(
        " user ",
        "prefers",
        " dark roast coffee ",
    ));
    assert_eq!(claim.subject(), "user", "parts are trimmed");
    assert_eq!(claim.object(), "dark roast coffee");
    assert_eq!(claim.as_key(), "user|prefers|dark roast coffee");

    assert_eq!(
        StructuredClaim::new("", "prefers", "coffee"),
        Err(InvalidMemory::ClaimPart)
    );
    assert_eq!(
        StructuredClaim::new("user", "prefers", "   "),
        Err(InvalidMemory::ClaimPart)
    );
    assert_eq!(
        StructuredClaim::new("user", "prefers", "x".repeat(MAX_CLAIM_PART_CHARS + 1)),
        Err(InvalidMemory::ClaimPart)
    );
}

/// **A source excerpt hash is a SHA-256 digest or nothing, never other text.**
///
/// A hash is what links a claim to the supporting text without storing a second copy of possibly-sensitive
/// source content. A value that is not a digest would silently fail to match on a later comparison, which
/// reads as "the source changed" rather than "the hash was never a hash".
#[test]
fn a_source_excerpt_hash_must_be_a_digest() {
    let digest = "a".repeat(64);
    let source = must(
        MemorySource::of_kind(MemorySourceKind::Document, "document:report.pdf")
            .and_then(|source| source.with_excerpt_hash(digest.clone())),
    );
    assert_eq!(source.excerpt_hash(), Some(digest.as_str()));

    for rejected in [
        String::new(),
        "abc".to_owned(),
        "A".repeat(64),
        "g".repeat(64),
        digest.repeat(2),
    ] {
        assert_eq!(
            MemorySource::new(
                MemorySourceKind::Document,
                "document:report.pdf",
                MemoryTrust::Derived,
                Some(rejected.clone()),
            ),
            Err(InvalidMemory::SourceExcerptHash),
            "{rejected:?} is not a digest"
        );
    }

    // An empty or oversized locator is refused: it is what "why was this remembered" resolves through.
    assert_eq!(
        MemorySource::of_kind(MemorySourceKind::Document, "   "),
        Err(InvalidMemory::Source)
    );
    assert_eq!(
        MemorySource::of_kind(
            MemorySourceKind::Document,
            "x".repeat(MAX_SOURCE_LOCATOR_CHARS + 1)
        ),
        Err(InvalidMemory::Source)
    );
}

/// **A retrieved memory records that it was useful, without pretending it was edited.**
///
/// The reinforcement signal is separate from `updated_at`, so "when was this last changed" stays answerable.
#[test]
fn retrieval_is_recorded_without_touching_updated_at() {
    let memory = preference();
    let retrieved = memory.retrieved(at(3));
    assert_eq!(retrieved.last_accessed_at(), Some(at(3)));
    assert_eq!(retrieved.retrieval_count(), 1);
    assert_eq!(
        retrieved.updated_at(),
        memory.updated_at(),
        "being read is not being edited"
    );
    assert_eq!(retrieved.content(), memory.content());
}

/// **Importance is clamped to its declared rank.**
#[test]
fn importance_is_clamped_to_its_rank() {
    let mut candidate = parts(MemoryType::Semantic, user_source());
    candidate.importance = u8::MAX;
    assert_eq!(
        must(MemoryRecord::new(candidate)).importance(),
        MAX_MEMORY_IMPORTANCE
    );
}

/// **Only the durable types are expected to outlive the run that produced them.**
///
/// The distinction a retention policy needs: a working memory scoped to one run and a conversation memory
/// scoped to a session are both collectable without anyone asking, and treating all seven alike would
/// either keep per-run scratch forever or expire a confirmed preference.
#[test]
fn only_durable_types_survive_the_run() {
    assert!(!MemoryType::Working.is_durable());
    assert!(!MemoryType::Conversation.is_durable());
    for durable in [
        MemoryType::Episodic,
        MemoryType::Semantic,
        MemoryType::Preference,
        MemoryType::Relationship,
        MemoryType::Procedural,
    ] {
        assert!(durable.is_durable(), "{durable} must be durable");
    }
    assert!(MemoryType::Relationship.requires_confirmation());
    assert!(!MemoryType::Semantic.requires_confirmation());
}

/// **A memory's default source kind and trust fail closed.**
///
/// The defaults are what a deserialiser fills in when a field is absent, and a memory whose origin was
/// never recorded must not be treated as something the user said.
#[test]
fn an_unrecorded_origin_defaults_to_untrusted_external() {
    assert_eq!(
        MemorySourceKind::default(),
        MemorySourceKind::ExternalContent
    );
    assert_eq!(MemoryTrust::default(), MemoryTrust::Untrusted);
    assert!(
        !MemoryTrust::Untrusted.may_instruct(),
        "untrusted content is data, never instruction"
    );
    assert!(MemoryTrust::Authoritative.may_instruct());
}
