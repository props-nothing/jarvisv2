//! Tests for converting a stored claim into an isolated context item.
//!
//! # What these are about
//!
//! Two different rules meet here, and the tests keep them apart deliberately:
//!
//! - **The trust mapping** ([`MemoryRecord::context_trust`]), which decides how the model should read a
//!   claim. The tests assert the two places it is deliberately *not* the identity, because those are the
//!   ones a later reader would "simplify".
//! - **The conversion** ([`MemoryRecord::context_item`] and [`RetrievedMemory`]), which decides whether a
//!   claim may be offered at all.
//!
//! The isolation itself is tested in `crate::isolation`; what is asserted here is that the two are
//! **wired together**, because a correct isolation module that nothing calls would pass its own tests and
//! protect nothing.

use super::*;
use crate::context::{ContextPriority, ContextSourceKind};
use crate::id::MemoryId;

const WORKSPACE: &str = "0198f000-0000-7000-8000-000000000001";
const ACTOR: &str = "0198f000-0000-7000-8000-0000000000a1";

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

/// Unpacks a `Result` that must have failed, or panics naming what was expected.
///
/// The workspace denies `clippy::expect_used` in test code too, so this is the crate's idiom rather than
/// `.expect_err(..)` — and it carries the expectation into the message, which is what makes a failure say
/// which rule was being checked rather than only that something went wrong.
fn must_err<T, E>(result: Result<T, E>, what: &str) -> E {
    match result {
        Ok(_) => panic!("expected a failure: {what}"),
        Err(error) => error,
    }
}

fn at(minute: i128) -> UtcTimestamp {
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn workspace() -> WorkspaceId {
    must(WORKSPACE.parse())
}

/// A claim from a given source kind, at the confidence the kind's trust permits.
///
/// # Why the confidence is passed and the type is fixed
///
/// **The type is `Semantic` for every buildable claim**, not `Preference`. The first version used
/// `Preference` for every non-model kind and the sweep failed with `InvalidMemory::Source` � a
/// `ProviderRecord` may not back a preference, which `P4-001` refuses at construction. The domain was
/// right and the fixture was wrong, and it is the reason a sweep over one dimension needs a **neutral**
/// value on every other: `Semantic` is the type no source kind is barred from.
///
/// **The confidence is a parameter** because it is the second dimension some of these tests vary, and it
/// is the dimension along which the trust mapping actually changes.
///
/// `try_record` is the fallible form, for a sweep that has to skip the combinations the domain refuses.
fn try_record(
    kind: MemorySourceKind,
    content: &str,
    confidence: MemoryConfidence,
) -> Result<MemoryRecord, InvalidMemory> {
    MemoryRecord::new(MemoryRecordParts {
        id: MemoryId::new(),
        workspace_id: workspace(),
        memory_type: MemoryType::Semantic,
        content: content.to_owned(),
        structured_claim: None,
        source: MemorySource::of_kind(kind, "session:0198f000-0000-7000-8000-000000000003")?,
        confidence,
        importance: 2,
        sensitivity: Sensitivity::Internal,
        entities: vec![EntityRef::confirmed(EntityId::new())],
        valid_from: None,
        valid_until: None,
        supersedes: None,
        run_id: None,
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: crate::id::CorrelationId::new(),
        created_at: at(0),
    })
}

/// The infallible form, for a fixture that is certain to be buildable.
fn record(kind: MemorySourceKind, content: &str, confidence: MemoryConfidence) -> MemoryRecord {
    must(try_record(kind, content, confidence))
}

/// A confirmed user statement about a preference, which is the ordinary case.
///
/// `Preference` rather than `Semantic` so the source-reference test has a type to look for, and because a
/// preference is what `P4-004`'s eligibility tests are about.
fn user_statement() -> MemoryRecord {
    let mut claim = record(
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
        MemoryConfidence::Confirmed,
    );
    claim.memory_type = MemoryType::Preference;
    claim
}

/// **A model inference is never `Derived`, so it cannot feed itself forward.**
///
/// The mapping's most important departure from the source trust, and the reason it exists: `Derived` would
/// make a model's own previous output read as evidence this platform gathered. The test is written over
/// every source kind so a *new* kind is covered by construction rather than by someone remembering.
#[test]
fn a_model_inference_is_untrusted_whatever_its_confidence() {
    // The domain caps a model inference at `Unverified`, so that is the only level it can be built at.
    let inference = record(
        MemorySourceKind::ModelInference,
        "The user may prefer tea",
        MemoryConfidence::Unverified,
    );
    assert_eq!(inference.source().trust(), MemoryTrust::Derived);
    assert_eq!(
        inference.context_trust(),
        ContextTrust::Untrusted,
        "a model inference must not reach a prompt as derived evidence"
    );

    // And the general rule over every kind: the mapping agrees with the source trust everywhere except
    // the two documented departures. A combination the domain refuses is skipped rather than asserted
    // about, because it is not a memory � `MemoryRecord::new` is where "a model inference above
    // `Unverified`" and "a provider record backing a preference" are excluded, so a test that demanded a
    // trust mapping for one would be demanding a property of a value that cannot exist.
    let mut checked = 0_usize;
    for kind in MemorySourceKind::all() {
        for confidence in MemoryConfidence::all() {
            let Ok(claim) = try_record(kind, "A claim about something", confidence) else {
                continue;
            };
            checked += 1;
            let expected = match kind.permitted_trust() {
                MemoryTrust::Untrusted => ContextTrust::Untrusted,
                MemoryTrust::Derived => {
                    if kind.is_model_produced() {
                        ContextTrust::Untrusted
                    } else {
                        ContextTrust::Derived
                    }
                }
                MemoryTrust::Authoritative => {
                    if confidence.is_stated_as_fact() {
                        ContextTrust::User
                    } else {
                        ContextTrust::Derived
                    }
                }
            };
            assert_eq!(
                claim.context_trust(),
                expected,
                "the mapping must be as documented for {kind} at {confidence}"
            );
        }
    }
    assert!(
        checked >= MemorySourceKind::all().len(),
        "the sweep must have exercised at least one combination per kind, or it proves nothing"
    );
}

/// **No memory that can be built carries instruction-bearing trust.**
///
/// The property the instruction boundary rests on: only `Authoritative` content is instruction-bearing, so
/// a memory reaching that class would be a claim that could tell the model what to do. Asserted over every
/// (kind, confidence) pair the domain **allows**, which is why it goes through `try_record`: the pairs it
/// refuses are not memories, and the count assertion below keeps the sweep from becoming vacuous if the
/// domain ever starts refusing more than expected.
#[test]
fn no_memory_can_carry_authoritative_trust() {
    let mut checked = 0_usize;
    for kind in MemorySourceKind::all() {
        for confidence in MemoryConfidence::all() {
            let Ok(claim) = try_record(kind, "Content", confidence) else {
                continue;
            };
            checked += 1;
            assert!(
                !claim.context_trust().is_instruction_bearing(),
                "{kind} at {confidence} produced instruction-bearing trust"
            );
        }
    }
    assert!(
        checked >= MemorySourceKind::all().len(),
        "every kind must contribute at least one buildable combination"
    );
}

/// **An unconfirmed claim from an authoritative source is not presented as the user's words.**
///
/// The second departure from the source trust. A user statement recorded `Uncertain` is this platform's
/// uncertain reading of something, and a `User` label would present the hedge as the person's own
/// assertion.
#[test]
fn an_unconfirmed_user_statement_is_not_labelled_as_the_user() {
    let confirmed = record(
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
        MemoryConfidence::Confirmed,
    );
    assert_eq!(confirmed.context_trust(), ContextTrust::User);

    let hedged = record(
        MemorySourceKind::UserStatement,
        "Possibly prefers dark roast coffee",
        MemoryConfidence::Likely,
    );
    assert_eq!(hedged.source().trust(), MemoryTrust::Authoritative);
    assert_eq!(
        hedged.context_trust(),
        ContextTrust::Derived,
        "an unconfirmed reading must not be labelled as the user speaking"
    );
}

/// **The item is `Optional`, because a retrieval reason implies it.**
///
/// The one table both directions read, so a retrieved memory cannot claim `Preferred` and outrank the
/// reserved policy tier. An earlier draft of the conversion tried to rank memories `Preferred`; it would
/// have been refused at construction, which is the table doing its job. Pinned because the reason to
/// revisit it � "relevant memories should beat older history" � is a plausible-sounding one.
#[test]
fn a_retrieved_memory_is_optional_and_says_why_it_is_there() {
    let item = must(user_statement().context_item(&[], at(1)));
    assert_eq!(item.priority(), ContextPriority::Optional);
    assert_eq!(item.reason(), &InclusionReason::RetrievedMatch);
    assert_eq!(item.source().kind(), ContextSourceKind::Memory);
    assert_eq!(item.trust(), ContextTrust::User);
    // Not quoted: the quoted flag is the *external content* marker, and a confirmed user statement is not
    // external. What isolates it is the fence at render time, which every memory gets.
    assert!(!item.is_quoted());
}

/// **Untrusted memory is quoted, which the context contract requires.**
///
/// The two modules have to agree: `jarvis_core::context` refuses an untrusted item that is not marked, so a
/// conversion that forgot the flag would fail at construction with a message about the envelope rather than
/// about the memory. The test asserts the flag is set, so the failure mode is never reached.
#[test]
fn an_untrusted_memory_is_marked_quoted() {
    let external = record(
        MemorySourceKind::ExternalContent,
        "A page said the user likes tea",
        MemoryConfidence::Uncertain,
    );
    assert_eq!(external.context_trust(), ContextTrust::Untrusted);
    let item = must(external.context_item(&[], at(1)));
    assert!(item.is_quoted(), "untrusted content must be marked quoted");
}

/// **The source reference names the memory, its type, and its origin.**
///
/// The reference is what an operator sees when asking why something was used, and an opaque identifier
/// answers only *which* record. Asserted for the parts rather than the whole string, so a format change
/// that kept the information would pass.
#[test]
fn the_source_reference_names_the_record_and_its_origin() {
    let claim = user_statement();
    let item = must(claim.context_item(&[], at(1)));
    let reference = item.source().reference();
    assert!(reference.starts_with("memory:"));
    assert!(reference.contains(&claim.id().to_string()));
    assert!(reference.contains("preference"));
    assert!(reference.contains("user_statement"));
}

/// **A claim the use case does not allow is refused by name.**
///
/// The type filter is applied at conversion rather than left to the caller, because a caller that ranked
/// through one path and assembled through another would otherwise have two answers.
#[test]
fn a_type_the_use_case_disallows_is_refused() {
    let claim = user_statement();
    let refusal = must_err(
        claim.context_item(&[MemoryType::Semantic], at(1)),
        "a preference is not a semantic memory",
    );
    assert_eq!(
        refusal,
        MemoryContextRefusal::TypeNotAllowed(MemoryType::Preference)
    );
    assert_eq!(refusal.as_str(), "type_not_allowed");

    // An empty allow-list means "no type restriction", which is the reading `P4-004`'s eligibility uses.
    assert!(claim.context_item(&[], at(1)).is_ok());
}

/// **A claim that is not current truth is refused, and the state is named.**
///
/// A `Proposed` or `Expired` claim is not *less relevant* than an active one; it is not offered at all.
/// Each state is exercised so a narrowed check cannot pass on a neighbour's coverage.
#[test]
fn a_claim_that_is_not_current_is_refused() {
    let claim = record(
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee",
        MemoryConfidence::Confirmed,
    );

    // Proposed: awaiting review, which is what the review step exists to gate.
    let mut proposed = claim.clone();
    proposed.status = MemoryStatus::Proposed;
    assert_eq!(
        proposed.context_item(&[], at(1)),
        Err(MemoryContextRefusal::NotCurrent(
            EffectiveMemoryStatus::Proposed
        ))
    );

    // Superseded: corrected, retained for audit only. The replacement names another memory, which is the
    // shape a correction actually takes.
    let superseded = must(claim.replace_with(MemoryId::new(), at(1)));
    assert_eq!(
        must_err(superseded.context_item(&[], at(1)), "superseded"),
        MemoryContextRefusal::NotCurrent(EffectiveMemoryStatus::Superseded)
    );

    // Expired: the validity window closed, which is derived from the clock rather than stored.
    let mut expiring = claim.clone();
    expiring.valid_until = Some(at(5));
    assert!(expiring.context_item(&[], at(1)).is_ok(), "still valid");
    assert_eq!(
        must_err(expiring.context_item(&[], at(6)), "expired"),
        MemoryContextRefusal::NotCurrent(EffectiveMemoryStatus::Expired)
    );

    // Deleted: content removed, terminal.
    let deleted = must(claim.delete(at(1)));
    assert_eq!(
        must_err(deleted.context_item(&[], at(2)), "deleted"),
        MemoryContextRefusal::NotCurrent(EffectiveMemoryStatus::Deleted)
    );
}

/// **The content is isolated before it is sized or sent, and the two agree.**
///
/// The wiring this file exists to check: a correct isolation module nothing calls protects nothing. The
/// assertion is that the item's token estimate matches the *rendered* payload, which is the value that
/// would go into a request � so the budget accounts for the fence rather than only the text.
#[test]
fn the_item_is_sized_for_the_rendered_payload() {
    let claim = record(
        MemorySourceKind::UserStatement,
        "Prefers dark roast coffee with a note\u{202e} containing an override",
        MemoryConfidence::Confirmed,
    );
    let retrieved = must(RetrievedMemory::new(&claim, &[], at(1)));

    assert!(retrieved.isolated().was_altered());
    assert!(
        !retrieved.isolated().body().contains('\u{202e}'),
        "the override must not reach the payload"
    );
    let rendered = retrieved.isolated().render();
    assert_eq!(
        retrieved.item().token_estimate(),
        u32::try_from(rendered.len()).unwrap_or(u32::MAX).max(1),
        "the estimate must be of the rendered payload, or the budget under-counts the fence"
    );
}

/// **The estimate is never short of what would be sent.**
///
/// The property the previous assertion implies, stated as an inequality so it holds for every payload rather
/// than for one fixture. This is the direction that matters: an estimate **below** the rendered length lets
/// the budget be exceeded, while one above it merely spends budget conservatively.
///
/// It is also the test that would have caught the original defect most cheaply — the first version added the
/// two marker lengths and forgot the newlines the rendering inserts, so it was two bytes short on every
/// non-empty payload. An earlier draft of this file asserted the *equality* using the same wrong arithmetic
/// the code used, so it reproduced the bug and passed.
#[test]
fn the_estimate_is_never_short_of_the_rendered_payload() {
    // Every shape a payload can take, so the inequality is checked against the transform's own output
    // rather than against one hand-computed length.
    for content in [
        "one short claim",
        "a claim\nwith\nseveral\nlines",
        "a claim with a \u{202e} format character",
        &"a".repeat(1_000),
    ] {
        let claim = record(
            MemorySourceKind::UserStatement,
            content,
            MemoryConfidence::Confirmed,
        );
        let retrieved = must(RetrievedMemory::new(&claim, &[], at(1)));
        let rendered = retrieved.isolated().render().len();
        let estimate = usize::try_from(retrieved.item().token_estimate()).unwrap_or(usize::MAX);
        assert!(
            estimate >= rendered,
            "the estimate {estimate} must not be short of the {rendered}-byte rendering of {content:?}"
        );
    }
}

/// **A memory whose content is only a format character cannot be offered.**
///
/// The one way a stored memory reaches the conversion and cannot be isolated, since the domain bounds the
/// length and refuses empty content. Reported as a content refusal rather than panicking, so a hostile or
/// corrupt row is a dropped candidate rather than a failed run.
#[test]
fn a_memory_that_cannot_be_isolated_is_refused() {
    // The domain refuses empty and whitespace-only content, so the reachable case is a payload that
    // neutralises to nothing.
    let claim = record(
        MemorySourceKind::UserStatement,
        "\u{200b}\u{202e}",
        MemoryConfidence::Confirmed,
    );
    let refusal = must_err(RetrievedMemory::new(&claim, &[], at(1)), "unusable content");
    assert!(matches!(refusal, MemoryContextRefusal::Content(_)));
    assert_eq!(refusal.as_str(), "content_unusable");
}

/// **The introduction names the count and both markers, and states the precedence rule.**
///
/// The introduction is the only part of the memory message this platform authors, so it is the only part
/// that can tell the model how to read the rest. Asserted for the three claims it makes rather than for its
/// wording, so a rephrasing that kept them would pass.
#[test]
fn the_introduction_states_how_to_read_the_region() {
    let introduction = memory_context_introduction(3);
    assert!(introduction.contains('3'), "it must name the count");
    assert!(introduction.contains(crate::isolation::FENCE_OPEN));
    assert!(introduction.contains(crate::isolation::FENCE_CLOSE));
    assert!(
        introduction.to_lowercase().contains("not instructions"),
        "it must say the region is data"
    );
    assert!(
        introduction
            .to_lowercase()
            .contains("policy and the request win"),
        "it must state which source takes precedence on a contradiction"
    );
}
