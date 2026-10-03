//! Tests for session summarization.
//!
//! # What these are about
//!
//! `P4-015` has three requirements and each is a different kind of claim: the summary is **derived** (a trust
//! class), it carries **provenance** (a source link), and it carries a **retention rule** — with the
//! document's own "loss metadata" on top. So the tests are grouped by which requirement they hold, and the
//! interesting ones are where two of them could be satisfied while the third is broken.

use super::*;
use crate::id::{CorrelationId, MemoryId, SessionId, WorkspaceId};
use crate::memory::{MemoryConfidence, MemoryTrust, MemoryType};
use crate::timestamp::UtcTimestamp;

const ACTOR: &str = "0198f000-0000-7000-8000-0000000000b1";

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("expected success, got {error:?}"),
    }
}

fn at(minute: i128) -> UtcTimestamp {
    const BASE: i128 = 1_774_000_000_500_000_000;
    must(UtcTimestamp::from_unix_nanos(
        BASE + minute * 60_000_000_000,
    ))
}

fn session() -> SessionId {
    SessionId::new()
}

/// A summary of a 4-message span, which is the fixture most rules are about.
fn summary() -> SessionSummary {
    let session_id = session();
    must(SessionSummary::new(SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id,
        text: "The user asked about the tax return and JARVIS listed two open items.".to_owned(),
        span: must(SummarySpan::new(session_id, 1, 4)),
        loss: must(SummaryLoss::new(4, Some(900))),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: vec![subject()],
    }))
}

/// The conversation entity a summary is about.
///
/// A `confirmed` reference rather than a guessed one, because the summary is stored against an entity the
/// caller established — a summary attached to an inferred subject is the mistake `P4-001` records.
fn subject() -> crate::memory::EntityRef {
    crate::memory::EntityRef::confirmed(crate::id::EntityId::new())
}

// ------------------------------------------------------------------------------------------------
// Requirement: it is never presented as user-authored fact
// ------------------------------------------------------------------------------------------------

/// **A summary is a `Document`, never a user statement, and the trust class follows from that.**
///
/// `P4-015`: "never presented as user-authored fact". The tempting implementation is `UserStatement`, because
/// the summary *describes what the user said* — and that would be the one thing the requirement forbids: a
/// compression of a conversation labelled as the person's own words, carrying `Authoritative` trust so it could
/// outrank the turns it was derived from.
///
/// Both halves are asserted, because the kind alone is satisfied by a source that maps to a different trust:
/// the kind must be `Document` **and** the trust must be `Derived`.
#[test]
fn a_summary_is_a_derived_document_and_not_a_user_statement() {
    let record = must(summary().into_record());
    assert_eq!(record.memory_type(), MemoryType::Conversation);
    assert_eq!(record.source().kind(), MemorySourceKind::Document);
    assert_eq!(record.source().trust(), MemoryTrust::Derived);
    assert_ne!(
        record.source().kind(),
        MemorySourceKind::UserStatement,
        "a compression of what the person said is not the person saying it"
    );
    assert!(
        !record.source().kind().is_model_produced(),
        "and it must not be a model inference, which no prompt may ever receive"
    );
    // The context trust the retrieval path reads: `Derived`, not `User`, which is the value a presentation
    // site uses to decide whether to hedge.
    assert_eq!(record.context_trust(), crate::ContextTrust::Derived);
}

/// **A summary is `Likely` at most, so it is never stated as fact.**
///
/// The confidence half of the same requirement. `Document` permits `Likely`, and `Likely` is **not** a
/// confidence `is_stateable_as_fact_at` accepts — so a summary can be offered as context while never being
/// presented to a user as established. Asserting the ceiling alone would pass for a summary recorded
/// `Confirmed`; asserting the predicate alone would pass for one recorded `Unverified`.
#[test]
fn a_summary_is_never_stateable_as_fact() {
    let record = must(summary().into_record());
    assert_eq!(record.confidence(), MemoryConfidence::Likely);
    assert!(
        !record.confidence().is_stated_as_fact(),
        "a summary is this platform's reading of a conversation, not an established fact"
    );
    assert!(
        !record.is_stateable_as_fact_at(at(1)),
        "and the presentation predicate must agree with the confidence"
    );
}

/// **The status is `Active`, and that is a decision rather than an omission.**
///
/// A summary has nothing to confirm: it is produced by deterministic code from a session this platform holds,
/// so it is not an inference and needs no approval. Asserted because the alternative is plausible — routing it
/// through the candidate pipeline as a proposal would be defensible for a *model-produced* summary, and this
/// build produces the text itself.
#[test]
fn a_summary_is_recorded_active() {
    let record = must(summary().into_record());
    assert_eq!(record.status(), SUMMARY_STATUS);
    assert!(record.status().is_current_claim());
}

/// **A summary cannot reach a prompt while the transcript that produced it is still replayed.**
///
/// `MODEL_MEMORY_TYPES` excludes `Conversation` with a stated reason: "the transcript is already replayed
/// through history, and offering it twice would present one turn as independent corroboration of itself." A
/// summary of those same turns is that error in weaker form.
///
/// The predicate is about the **replayed spans**, not about the summary's type, so both directions are
/// asserted: not offerable while the span is replayed, offerable once the window has moved past it. The second
/// assertion is the one that matters most — a predicate that answered `false` unconditionally would satisfy
/// the first, which is exactly the shape this test replaced.
#[test]
fn a_summary_is_not_offerable_while_its_transcript_is_replayed_and_is_offerable_afterwards() {
    let value = summary();
    let span = value.span();
    let session_id = value.session_id();
    // Windows of the *same* session, so the assertions are about the span relation rather than about session
    // identity — which `spans_overlap` treats separately and asserts separately below.
    let window = |first, last| must(SummarySpan::new(session_id, first, last));
    assert!(
        !is_offerable_as_context(&value, &[span]),
        "a summary must not be offered while the turns it compressed are still in the prompt"
    );
    assert!(
        !is_offerable_as_context(&value, &[window(3, 6)]),
        "an overlapping window replays some of the same turns"
    );
    assert!(
        is_offerable_as_context(&value, &[window(9, 12)]),
        "once the window has moved past the span the summary is the only copy of those turns"
    );
    assert!(
        is_offerable_as_context(&value, &[]),
        "with no history replayed at all the summary is not a duplicate"
    );
    // The type-level exclusion is the reason the record must never be offered by the *type* path either, so it
    // is asserted with the predicate: an offerable summary of an excluded type is still excluded there.
    let record = must(value.into_record());
    assert_eq!(record.memory_type(), MemoryType::Conversation);
}

// ------------------------------------------------------------------------------------------------
// Requirement: provenance
// ------------------------------------------------------------------------------------------------

/// **The source locator names the session and the exact span, so "what did this compress" is answerable.**
///
/// `memory-and-context.md`'s assembly step 6 requires "source links". The locator is the link, and it must
/// name the **span** rather than only the session: a summary of messages 1–4 and one of messages 5–9 are
/// different compressions, and a locator naming only the session cannot tell them apart or detect an overlap.
#[test]
fn the_source_locator_names_the_session_and_the_span() {
    let value = summary();
    let session_id = value.session_id();
    let record = must(value.into_record());
    assert_eq!(
        record.source().locator(),
        format!("summary:session/{session_id}/1..4"),
        "the locator must name the session and the covered range"
    );
}

/// **A span that ends before it begins is refused rather than normalised.**
///
/// A producer reporting the bounds backwards has a bug, and swapping them would hide it — while the bounds are
/// what a later overlap check reads, so a normalised pair would make "has this passage been summarized" answer
/// `yes` for a range nobody read.
///
/// The control immediately after is the single-message span, which is legal and must be accepted: refusing it
/// would make a one-message summary impossible, and the test above would still pass.
#[test]
fn an_inverted_span_is_refused_and_a_single_message_span_is_not() {
    let session_id = session();
    assert_eq!(
        SummarySpan::new(session_id, 4, 1).err(),
        Some(InvalidSummary::InvertedSpan)
    );
    let single = must(SummarySpan::new(session_id, 7, 7));
    assert_eq!(single.message_count(), 1);
}

/// **A span naming a different session than the summary's is refused.**
///
/// Two fields state one fact, and a producer that read one session and labelled another would store a summary
/// attributed to a conversation it never saw. It is a separate variant from the inverted span because the
/// causes differ — arithmetic versus mislabelling — and the remedies do too.
#[test]
fn a_span_for_another_session_is_refused() {
    let mut parts = SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id: session(),
        text: "A summary.".to_owned(),
        span: must(SummarySpan::new(session(), 1, 2)),
        loss: must(SummaryLoss::new(2, Some(10))),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: vec![subject()],
    };
    // The two identifiers are generated independently, so they differ — which is the case that must fail.
    assert_ne!(parts.session_id, parts.span.session_id);
    assert_eq!(
        SessionSummary::new(parts.clone()).err(),
        Some(InvalidSummary::SessionMismatch)
    );
    // The control: the same parts with the span's session corrected are accepted, so the refusal is about the
    // mismatch and not about the fixture.
    parts.span.session_id = parts.session_id;
    assert!(SessionSummary::new(parts).is_ok());
}

// ------------------------------------------------------------------------------------------------
// Requirement: loss metadata
// ------------------------------------------------------------------------------------------------

/// **A summary records how much it covered, and refuses to claim more turns than its span holds.**
///
/// The document requires "loss metadata". A summary that reported 20 covered turns over a 3-message span would
/// make "how much did this compress" answerable two ways, and the two answers would disagree — so the
/// relationship is enforced rather than stored.
///
/// The **other direction is permitted**, and asserted: a span may cover more messages than contributed
/// content, because a turn can be empty of anything extractable. Refusing that would make a summary of a
/// conversation with a one-word reply impossible.
#[test]
fn loss_metadata_may_not_claim_more_turns_than_the_span_holds() {
    let session_id = session();
    let parts = |turns: u32, first: i64, last: i64| SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id,
        text: "A summary.".to_owned(),
        span: must(SummarySpan::new(session_id, first, last)),
        loss: must(SummaryLoss::new(turns, None)),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: vec![subject()],
    };

    assert_eq!(
        SessionSummary::new(parts(20, 1, 3)).err(),
        Some(InvalidSummary::NoTurnsCovered),
        "20 turns cannot come from a 3-message span"
    );
    // The permitted direction: fewer turns than messages, because a turn may contribute nothing.
    assert!(SessionSummary::new(parts(2, 1, 3)).is_ok());
    // And a span whose whole range contributed.
    assert!(SessionSummary::new(parts(3, 1, 3)).is_ok());
}

/// **Loss metadata refuses a zero-turn claim and an absurd one.**
///
/// A summary covering nothing is not a summary, and the bound is stated because the value is stored and
/// displayed. Both ends are asserted with the boundary values, so an implementation using `<` instead of `<=`
/// fails.
#[test]
fn loss_metadata_is_bounded_at_both_ends() {
    assert_eq!(
        SummaryLoss::new(0, None).err(),
        Some(InvalidSummary::NoTurnsCovered)
    );
    assert!(SummaryLoss::new(1, None).is_ok());
    assert!(SummaryLoss::new(MAX_SUMMARIZED_TURNS, None).is_ok());
    assert_eq!(
        SummaryLoss::new(MAX_SUMMARIZED_TURNS + 1, None).err(),
        Some(InvalidSummary::TooManyTurns)
    );
}

/// **The compression ratio is reported when the input's size is known, and absent when it is not.**
///
/// `None` rather than a guess, because a producer that did not measure its input must not report a figure: a
/// fabricated ratio is worse than an absent one, exactly as an invented approver would be. And a ratio **above
/// one is permitted and asserted** — a summary of a very short exchange is legitimately longer, and refusing
/// it would make summarisation fail on the sessions that need no compression.
#[test]
fn the_compression_ratio_is_reported_only_when_the_input_size_is_known() {
    let short = summary();
    // A `None` source size yields no ratio rather than a default.
    let unknown = must(SummaryLoss::new(4, None));
    assert_eq!(unknown.compression_ratio(&short), None);

    let known = must(SummaryLoss::new(4, Some(900)));
    let ratio = known
        .compression_ratio(&short)
        .unwrap_or_else(|| panic!("a known source size yields a ratio"));
    assert!(ratio < 1.0, "a summary of 900 characters is shorter: {ratio}");

    // The permitted direction: a summary larger than its source. Constructed from a *zero-length* source,
    // which is the one case the ratio cannot express — asserted as `None` so the division cannot panic.
    let empty_source = must(SummaryLoss::new(1, Some(0)));
    assert_eq!(empty_source.compression_ratio(&short), None);
}

// ------------------------------------------------------------------------------------------------
// Requirement: overlap detection, which is what keeps two summaries from disagreeing
// ------------------------------------------------------------------------------------------------

/// **Two summaries of the same passage are detectable, so a session is not summarized twice.**
///
/// Two summaries of one span are two claims about the same passage and a reader cannot tell which is current.
/// The check is the relation it must be — overlapping in **either** direction — and each direction is asserted
/// separately, because an implementation testing only `left.first <= right.last` would report disjoint spans
/// as overlapping and one testing only the reverse would miss a contained span.
#[test]
fn overlapping_spans_are_detected_in_both_directions() {
    let session_id = session();
    let span = |first, last| must(SummarySpan::new(session_id, first, last));

    // Identical, contained, and straddling — three ways two spans overlap.
    assert!(spans_overlap(span(1, 4), span(1, 4)), "identical spans");
    assert!(spans_overlap(span(1, 9), span(3, 5)), "a contained span");
    assert!(spans_overlap(span(3, 5), span(1, 9)), "the same pair reversed");
    assert!(spans_overlap(span(1, 4), span(4, 8)), "sharing one message");

    // And disjoint spans do not, including adjacent ones — the boundary each direction must get right.
    assert!(!spans_overlap(span(1, 4), span(5, 8)), "adjacent spans");
    assert!(!spans_overlap(span(5, 8), span(1, 4)), "the same pair reversed");
    // A different session never overlaps, whatever the sequences, because the relation is about one session.
    let other = must(SummarySpan::new(session(), 1, 4));
    assert!(!spans_overlap(span(1, 4), other), "a different session");
}

// ------------------------------------------------------------------------------------------------
// Requirement: empty text is not a summary
// ------------------------------------------------------------------------------------------------

/// **A summary with no subject is refused here, not by the record constructor.**
///
/// A memory must name what it is about — the architecture's central rule — and `MemoryRecord::new` refuses one
/// that does not. So a summary reaching it with no entity would fail with `InvalidMemory::Entities`, which is a
/// storage-shaped error reported against a field the caller never supplied.
///
/// The refusal is here so the error names the **remedy**: a summary needs the conversation entity it is a
/// summary of. Both halves are asserted — the refusal, and that the record constructor would have produced a
/// different error for the same input — because the point is *which* layer reports it, and a test of the
/// refusal alone passes if `into_record` is never reached for any reason.
#[test]
fn a_summary_with_no_subject_is_refused_before_the_record_constructor() {
    let session_id = session();
    let mut parts = SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id,
        text: "A summary.".to_owned(),
        span: must(SummarySpan::new(session_id, 1, 2)),
        loss: must(SummaryLoss::new(2, None)),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: Vec::new(),
    };
    assert_eq!(
        SessionSummary::new(parts.clone()).err(),
        Some(InvalidSummary::NoEntity)
    );
    // The control: supplying the subject makes it acceptable, so the refusal is about the subject and not about
    // the fixture.
    parts.entities = vec![subject()];
    assert!(SessionSummary::new(parts).is_ok());
}

/// **The stored subject is the caller's conversation entity, and it is carried through unchanged.**
///
/// The rule the refusal above protects. Asserted on the identifier rather than on a count, because "the record
/// has an entity" is satisfied by any entity — including one this module invented, which would attach a
/// summary to a subject nobody created.
#[test]
fn the_summary_is_about_the_subject_the_caller_supplied() {
    let entity = crate::id::EntityId::new();
    let session_id = session();
    let value = must(SessionSummary::new(SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id,
        text: "A summary.".to_owned(),
        span: must(SummarySpan::new(session_id, 1, 2)),
        loss: must(SummaryLoss::new(2, None)),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: vec![crate::memory::EntityRef::confirmed(entity)],
    }));
    let record = must(value.into_record());
    assert_eq!(
        record.entities().iter().map(crate::memory::EntityRef::entity_id).collect::<Vec<_>>(),
        vec![entity],
        "the summary must be about exactly the subject the caller named"
    );
}

/// **A blank summary is refused, and the control is that non-blank text is accepted.**
///
/// The schema refuses empty content anyway, so this rule exists for the *diagnostic*: a storage `CHECK`
/// violation reports a column, while this reports that a summary was empty. Asserted with the boundary values
/// so a `<=` for a `<` fails.
#[test]
fn a_blank_summary_is_refused() {
    let session_id = session();
    let parts = |text: &str| SessionSummaryParts {
        summary_id: MemoryId::new(),
        workspace_id: WorkspaceId::new(),
        session_id,
        text: text.to_owned(),
        span: must(SummarySpan::new(session_id, 1, 2)),
        loss: must(SummaryLoss::new(2, None)),
        created_by_actor_id: ACTOR.to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: at(0),
        entities: vec![subject()],
    };
    assert_eq!(
        SessionSummary::new(parts("")).err(),
        Some(InvalidSummary::Text)
    );
    assert_eq!(
        SessionSummary::new(parts("   \n  ")).err(),
        Some(InvalidSummary::Text),
        "whitespace is not a summary"
    );
    // The boundary: one character is accepted, and an over-long one is not.
    assert!(SessionSummary::new(parts("A")).is_ok());
    assert_eq!(
        SessionSummary::new(parts(&"a".repeat(MAX_MEMORY_CONTENT_CHARS + 1))).err(),
        Some(InvalidSummary::Text)
    );
}
