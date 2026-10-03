//! Session summarization: a compressed session, stored as derived memory and never as a fact.
//!
//! `P4-015`: "a compressed summary of a session is stored as a `Derived` claim with provenance and a
//! retention rule, and is never presented as user-authored fact."
//!
//! # Why this is `Conversation` + `Document` + `Derived`, and each choice is load-bearing
//!
//! A summary has to be one of the existing types and kinds, because the schema's `IN` lists and the
//! source-kind/trust `CHECK` make a new member a **table rebuild** — and `P4-014` recorded the same conclusion
//! for the admission columns. Two of the three answers here are chosen for a reason worth stating, because a
//! plausible alternative gets one requirement wrong:
//!
//! - **`MemoryType::Conversation`.** The document's own table says this type is "dialogue continuity and
//!   **summaries** | session retention policy", so the type is the document's answer rather than ours. Its
//!   documented retention is a session policy, which is the retention rule the slice asks for.
//! - **`MemorySourceKind::Document`, not `UserStatement` and not `ModelInference`.** The summary is a
//!   *document this platform wrote by compressing a session*:
//!   - `UserStatement` would present it as the user's own words, which is exactly what "never presented as
//!     user-authored fact" forbids, and it would carry `Authoritative` trust so a summary could outrank the
//!     turns it compressed.
//!   - `ModelInference` is the tempting one, since a model will produce the text, and it is wrong because a
//!     `ModelInference` is **refused at any status** for prompt use (`ADR-0049` §6) — so a summary could never
//!     be offered as context, which is the only thing it is for.
//!
//!   `Document` is `Derived` trust and is prompt-eligible. That pair is the whole requirement: less than
//!   `User`, more than nothing.
//!
//! # Why `Conversation` is not currently offered to a model, and why that is deliberate
//!
//! `MODEL_MEMORY_TYPES` excludes `Conversation` with a stated reason: "the transcript is already replayed
//! through history, and offering it twice would present one turn as independent corroboration of itself." A
//! summary of those same turns would be that error in a weaker form. So a summary is **stored now and
//! offered when the transcript is no longer complete** — which is the problem summarisation exists to solve,
//! and which arrives with `P4-016`'s windowing. Recorded as a limit rather than papered over.
//!
//! # Loss metadata, which the document requires by name
//!
//! `memory-and-context.md`'s assembly step 6 is "compact or summarize **only with source links and loss
//! metadata**". Both are here and both are enforced:
//!
//! - **Source links.** The summary records the session and the exact spans it covers, as identifiers and
//!   sequence bounds rather than as text, so "what did this compress" is answerable without holding a second
//!   copy of the content.
//! - **Loss metadata.** The summary records how many turns it covers and whether the summary is shorter than
//!   what it replaced. A summary that claims to be complete when it dropped half the conversation is the
//!   failure this field exists to make visible.
//!
//! A summary with **no source span is refused** at construction, because that is the one property that cannot
//! be recovered afterwards: a compression whose input is unrecorded can never be checked, re-derived, or
//! corrected.

use crate::id::{MemoryId, SessionId, WorkspaceId};
use crate::memory::{
    MAX_MEMORY_CONTENT_CHARS, MemoryRecord, MemoryRecordParts, MemorySource, MemorySourceKind,
    MemoryStatus, MemoryType, InvalidMemory,
};
use crate::sensitivity::Sensitivity;
use crate::timestamp::UtcTimestamp;

/// The most turns one summary may claim to cover.
///
/// Bounded because the value is stored and displayed, and an unbounded count would let a summary assert a
/// coverage no session could have. A session longer than this needs more than one summary, which is the
/// correct answer rather than a larger bound.
pub const MAX_SUMMARIZED_TURNS: u32 = 100_000;

/// The fewest characters a summary must carry.
///
/// A non-empty bound rather than zero, so "the summary was empty" is a refusal at construction instead of a
/// memory with no content — which the schema refuses anyway, and a rule enforced in one place with a clear
/// name beats a `CHECK` violation reported as a storage error.
pub const MIN_SUMMARY_CHARS: usize = 1;

/// What a compression dropped, which the document requires be recorded.
///
/// # Why this is a struct rather than a single "is it lossy" flag
///
/// The document says "loss metadata", and the useful question is not *whether* information was lost but
/// **what was lost and how much**. A boolean answers the first and leaves an operator unable to tell a
/// summary of 12 turns from a summary of 400 — and a summary that quietly covered a tenth of its session is
/// the case the field exists for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SummaryLoss {
    /// How many source turns the summary covers.
    pub turns_covered: u32,
    /// Characters in the source spans the summary was derived from, when the producer knows.
    ///
    /// `None` rather than a guess: a producer that did not measure its input must not report a ratio, because
    /// a fabricated compression figure is worse than an absent one.
    pub source_chars: Option<usize>,
}

impl SummaryLoss {
    /// Builds loss metadata, refusing a summary that covers nothing.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSummary::NoTurnsCovered`] for a zero count, and
    /// [`InvalidSummary::TooManyTurns`] above [`MAX_SUMMARIZED_TURNS`].
    pub const fn new(turns_covered: u32, source_chars: Option<usize>) -> Result<Self, InvalidSummary> {
        if turns_covered == 0 {
            return Err(InvalidSummary::NoTurnsCovered);
        }
        if turns_covered > MAX_SUMMARIZED_TURNS {
            return Err(InvalidSummary::TooManyTurns);
        }
        Ok(Self {
            turns_covered,
            source_chars,
        })
    }

    /// Returns how much the summary shrank its input, when the input's size is known.
    ///
    /// A ratio below one means the summary is **larger** than the span it replaces, which is representable
    /// rather than refused: a summary of a very short exchange is legitimately longer, and refusing it would
    /// make summarisation fail on exactly the sessions that need no compression. Reporting it is what lets a
    /// caller decide not to store it.
    #[must_use]
    pub fn compression_ratio(&self, summary: &SessionSummary) -> Option<f64> {
        let source = self.source_chars?;
        if source == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        Some(summary.text.chars().count() as f64 / source as f64)
    }
}

/// The span of a session a summary covers.
///
/// # Why sequence bounds rather than a text digest
///
/// A digest of the input would prove *which* turns were read and would be useless to a reader: to check it
/// one must already have the turns. Sequence bounds name the range, so "what has this summary covered" is
/// answerable from the summary alone — which is what makes a later summary of the same session detectable as
/// overlapping rather than merely different.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SummarySpan {
    /// The session the span belongs to.
    pub session_id: SessionId,
    /// The sequence of the first covered message, inclusive.
    pub first_sequence: i64,
    /// The sequence of the last covered message, inclusive.
    pub last_sequence: i64,
}

impl SummarySpan {
    /// Builds a span, refusing an inverted or empty range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSummary::InvertedSpan`] when the last sequence precedes the first. Refused rather than
    /// normalised, because a producer that reported the bounds backwards has a bug a swap would hide, and the
    /// bounds are what a later overlap check reads.
    pub const fn new(
        session_id: SessionId,
        first_sequence: i64,
        last_sequence: i64,
    ) -> Result<Self, InvalidSummary> {
        if last_sequence < first_sequence {
            return Err(InvalidSummary::InvertedSpan);
        }
        Ok(Self {
            session_id,
            first_sequence,
            last_sequence,
        })
    }

    /// Returns how many messages the span covers, inclusive of both ends.
    #[must_use]
    pub const fn message_count(&self) -> i64 {
        self.last_sequence - self.first_sequence + 1
    }

    /// Returns whether every message this span names exists in a session of `session_message_count` messages.
    ///
    /// # Why the count arrives as an argument rather than being read here
    ///
    /// `jarvis-core` does not read storage. The count is what the transcript says it holds, read by the writer
    /// on the same connection as the write, and this is a *policy* about what a summary may claim. A span
    /// naming message 300 of a 12-message session produces a summary whose provenance points at turns that do
    /// not exist — and worse, `spans_overlap` would then suppress a later, correct summary of the real range,
    /// because the fabricated span overlaps it.
    ///
    /// # Why this counts rather than checks membership
    ///
    /// Sequences are allocated contiguously from zero and **nothing deletes a message** — a session's rows go
    /// only when the session does, by cascade. A count is therefore exact: the existing sequences are exactly
    /// `0 .. session_message_count`. Recorded as a limit rather than assumed silently: a row removed outside
    /// JARVIS would leave a gap this accepts, and the failure would be a summary whose provenance names an
    /// absent turn.
    ///
    /// The direction is stated in the name because it cannot be inferred from a name like `within`: this is
    /// true when the span is a **subset** of the session's messages, so `last` must be **strictly less** than
    /// the count — a span ending *at* the count names one message too many.
    #[must_use]
    pub const fn covers_only_existing_messages(&self, session_message_count: i64) -> bool {
        session_message_count > 0
            && self.last_sequence < session_message_count
            && self.first_sequence < session_message_count
    }
}

/// Why a summary could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidSummary {
    /// The summary text was empty or longer than a memory may hold.
    Text,
    /// The loss metadata claimed zero turns.
    NoTurnsCovered,
    /// The loss metadata claimed more turns than [`MAX_SUMMARIZED_TURNS`].
    TooManyTurns,
    /// The recorded span ended before it began.
    InvertedSpan,
    /// A span that covers a session but names no subject is refused.
    ///
    /// A memory must name what it is about, and `MemoryRecord::new` refuses one that does not — so this is the
    /// **same rule stated where it can name the remedy**. A summary that reached `into_record` with no entity
    /// would fail there with `InvalidMemory::Entities`, which reports a field rather than telling the caller
    /// that a summary needs a subject. The subject is normally the **conversation** entity
    /// (`EntityKind::Conversation`), because a summary is about a dialogue rather than about a person.
    NoEntity,
    /// The span's session disagreed with the summary's.
    ///
    /// A separate variant from [`Self::InvertedSpan`] because the two mistakes have different causes: one is
    /// arithmetic, the other is a producer that read one session and labelled another.
    SessionMismatch,
}

impl std::fmt::Display for InvalidSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => write!(
                formatter,
                "a summary must be {MIN_SUMMARY_CHARS} to {MAX_MEMORY_CONTENT_CHARS} characters"
            ),
            Self::NoTurnsCovered => {
                write!(formatter, "a summary must record at least one covered turn")
            }
            Self::TooManyTurns => write!(
                formatter,
                "a summary may claim at most {MAX_SUMMARIZED_TURNS} covered turns"
            ),
            Self::InvertedSpan => {
                write!(formatter, "a summary's covered span must not end before it begins")
            }
            Self::NoEntity => write!(
                formatter,
                "a summary must name what it is about; the subject is normally the conversation entity for \
                 its session"
            ),
            Self::SessionMismatch => {
                write!(formatter, "a summary's span must name the session it summarizes")
            }
        }
    }
}

impl std::error::Error for InvalidSummary {}

/// A compressed session, ready to store as a `Conversation` memory.
///
/// # Why this is not a `MemoryRecord`
///
/// A summary is the *input* to a memory, exactly as [`crate::MemoryCandidate`] is: it carries what the
/// producer knows (the text, the span, the loss) and none of what the store decides (the identifier, the
/// workspace, the actor, the clock). Keeping them apart means [`SessionSummary::into_record`] cannot invent a
/// provenance the caller did not state, and the caller cannot hand-build a row that skips the rules below.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    text: String,
    span: SummarySpan,
    loss: SummaryLoss,
    summary_id: MemoryId,
    workspace_id: WorkspaceId,
    created_by_actor_id: String,
    correlation_id: crate::id::CorrelationId,
    created_at: UtcTimestamp,
    entities: Vec<crate::memory::EntityRef>,
}

impl SessionSummary {
    /// Builds a summary, applying every rule this module states.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSummary::Text`] for empty or oversized text,
    /// [`InvalidSummary::SessionMismatch`] when the span names another session than the summary's, and the
    /// two loss-metadata refusals.
    pub fn new(parts: SessionSummaryParts) -> Result<Self, InvalidSummary> {
        let text = parts.text.trim();
        if text.chars().count() < MIN_SUMMARY_CHARS || text.chars().count() > MAX_MEMORY_CONTENT_CHARS {
            return Err(InvalidSummary::Text);
        }
        if parts.span.session_id != parts.session_id {
            return Err(InvalidSummary::SessionMismatch);
        }
        if parts.loss.turns_covered == 0 {
            return Err(InvalidSummary::NoTurnsCovered);
        }
        if parts.loss.turns_covered > MAX_SUMMARIZED_TURNS {
            return Err(InvalidSummary::TooManyTurns);
        }
        if parts.span.last_sequence < parts.span.first_sequence {
            return Err(InvalidSummary::InvertedSpan);
        }
        // **A summary must name what it is about, and the refusal is here rather than at the record.**
        //
        // `MemoryRecord::new` refuses an entity-less memory, so a summary reaching it with no subject would
        // fail with `InvalidMemory::Entities` — a storage-shaped error for a caller's omission, reported
        // against a field the caller never supplied. Checking here means the refusal names the remedy: a
        // summary needs the **conversation** entity it is a summary of.
        if parts.entities.is_empty() {
            return Err(InvalidSummary::NoEntity);
        }
        // A span narrower than the turn count it claims is refused, because the two values are two statements
        // of one fact: a producer reporting 20 turns over a 3-message span has a bug, and storing it would
        // make "how much did this compress" answerable two ways. The reverse is permitted — a span may cover
        // messages that contributed nothing, since a turn can be empty of extractable content.
        if parts.loss.turns_covered > u32::try_from(parts.span.message_count()).unwrap_or(u32::MAX) {
            return Err(InvalidSummary::NoTurnsCovered);
        }
        Ok(Self {
            text: text.to_owned(),
            span: parts.span,
            loss: parts.loss,
            summary_id: parts.summary_id,
            workspace_id: parts.workspace_id,
            created_by_actor_id: parts.created_by_actor_id,
            correlation_id: parts.correlation_id,
            created_at: parts.created_at,
            entities: parts.entities,
        })
    }

    /// Returns the summary text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the span this summary covers.
    #[must_use]
    pub const fn span(&self) -> SummarySpan {
        self.span
    }

    /// Returns what the compression dropped.
    #[must_use]
    pub const fn loss(&self) -> SummaryLoss {
        self.loss
    }

    /// Returns the session this summary belongs to.
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.span.session_id
    }

    /// Converts the summary into a memory record.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory`] when the record's own rules refuse the values — which they should not, since
    /// every bound here is the record's bound or tighter, so a failure means the two layers have diverged and
    /// is reported rather than swallowed.
    pub fn into_record(self) -> Result<MemoryRecord, InvalidMemory> {
        MemoryRecord::new(MemoryRecordParts {
            id: self.summary_id,
            workspace_id: self.workspace_id,
            // The document's own table: "Conversation | dialogue continuity and **summaries** | session
            // retention policy". The type is the document's answer rather than this module's, and its
            // retention rule is the session policy the slice asks for.
            memory_type: MemoryType::Conversation,
            content: self.text.clone(),
            structured_claim: None,
            // **`Document`, for the two reasons the module states.** `UserStatement` would present the summary
            // as the user's words; `ModelInference` would make it ineligible for every prompt, which is the
            // only thing a summary is for.
            source: MemorySource::of_kind(
                MemorySourceKind::Document,
                format!(
                    "summary:session/{}/{}..{}",
                    self.span.session_id, self.span.first_sequence, self.span.last_sequence
                ),
            )?,
            // `Likely`, the ceiling a `Document` permits: the summary faithfully reports what this platform
            // read, and it is not the user asserting anything. The pipeline caps it again, so this is the
            // value a caller states rather than a ceiling it can raise.
            confidence: crate::memory::MemoryConfidence::Likely,
            importance: SUMMARY_IMPORTANCE,
            // `Internal`, matching the type's floor. A summary of a private conversation must not be
            // disclosable to a public destination, and this module cannot know what the turns contained — so
            // the floor is the cautious value rather than a claim about the content.
            sensitivity: Sensitivity::Internal,
            // **The session's conversation entity, supplied by the caller.** A memory must name what it is
            // about, and a summary is about a dialogue rather than about a person — so the subject is the
            // `conversation` entity, which the architecture's own entity-kind list includes. It is supplied
            // rather than derived because this crate holds no store to resolve the session's entity against,
            // and inventing an identifier would attach the claim to a subject nobody created.
            entities: self.entities,
            valid_from: Some(self.created_at),
            valid_until: None,
            supersedes: None,
            run_id: None,
            created_by_actor_id: self.created_by_actor_id,
            correlation_id: self.correlation_id,
            created_at: self.created_at,
        })
    }
}

/// The importance every summary carries.
///
/// Two, the middle of the range, for the same reason a remembered claim's default is: a summary is a
/// convenience rather than a fact the user stated, and ranking it above explicit statements would let a
/// compression outrank its own source. It is not zero either — a summary is durable and should survive a
/// budget cut ahead of scratch state.
const SUMMARY_IMPORTANCE: u8 = 2;

/// The declared fields of a session summary.
///
/// # Why the identity and the clock are supplied rather than generated here
///
/// The same reason [`MemoryRecordParts`] takes them: a caller that is replaying or re-deriving a summary must
/// be able to use the same values, and a constructor that generated its own would make that impossible while
/// looking correct for the first write.
#[derive(Clone, Debug)]
pub struct SessionSummaryParts {
    /// The summary's own memory identifier.
    pub summary_id: MemoryId,
    /// The owning workspace, which is also the retrieval boundary.
    pub workspace_id: WorkspaceId,
    /// The session this summary covers.
    pub session_id: SessionId,
    /// The compressed text.
    pub text: String,
    /// The span of the session it covers.
    pub span: SummarySpan,
    /// What the compression dropped.
    pub loss: SummaryLoss,
    /// The actor that produced it, which for a platform-generated summary is the daemon's identity.
    pub created_by_actor_id: String,
    /// The correlation identity shared with the run that produced it.
    pub correlation_id: crate::id::CorrelationId,
    /// When it was produced.
    pub created_at: UtcTimestamp,
    /// The entities the summary is about, normally the **conversation** entity for its session.
    ///
    /// Required and non-empty: a summary with no subject is refused at construction rather than by the record
    /// constructor, so the error names what the caller must supply.
    pub entities: Vec<crate::memory::EntityRef>,
}

/// Returns whether a summary may be offered to a model as context.
///
/// # Why this takes the replayed spans rather than reading a stored flag
///
/// A summary is offerable exactly when the turns it compressed are **no longer replayed**, because a summary
/// and its transcript in one prompt present a passage twice — and the second copy reads as independent
/// corroboration of the first. That is the reason `MODEL_MEMORY_TYPES` excludes `Conversation`, and it is a
/// fact about the *session* rather than about the summary: it is false when the summary is written and becomes
/// true when the history window moves past it.
///
/// So the caller supplies what it knows about the prompt it is building. A stored flag was rejected because it
/// would record a decision made once and read forever, and this rule changes with the window.
///
/// # Why this is not a constant
///
/// The first version returned `false` unconditionally, which read as a decision and decided nothing: a
/// predicate whose answer cannot vary is the shape this repository calls "a count that is always zero". Taking
/// the replayed spans makes it the real rule, and gives [`spans_overlap`] the production caller it needs
/// rather than leaving it a function only a test used.
#[must_use]
pub fn is_offerable_as_context(summary: &SessionSummary, replayed: &[SummarySpan]) -> bool {
    !replayed
        .iter()
        .any(|span| spans_overlap(*span, summary.span))
}

/// Returns whether a later summary would overlap an existing one.
///
/// A span already covered must not be summarized again, because two summaries of the same turns are two
/// claims about one passage and a reader cannot tell which is current. Detecting it here rather than in SQL
/// keeps the rule stated once: `P4-016` calls this before writing.
#[must_use]
pub fn spans_overlap(left: SummarySpan, right: SummarySpan) -> bool {
    left.session_id == right.session_id
        && left.first_sequence <= right.last_sequence
        && right.first_sequence <= left.last_sequence
}

/// The status a stored summary must hold to be offered.
///
/// `Active`, never `Proposed`: a summary is produced by deterministic code from a session this platform
/// holds, so it has no inference to confirm and no approval to wait for. Stated here so a reader asking "why
/// is a summary not a proposal" has an answer beside the value rather than in a commit message.
pub const SUMMARY_STATUS: MemoryStatus = MemoryStatus::Active;

#[cfg(test)]
mod tests;
