//! Memory retrieval: eligibility before ranking, and a ranking that explains itself.
//!
//! `docs/architecture/memory-and-context.md` states retrieval as two stages and gives each a list. This
//! module owns both, as **pure functions over records**, so the rules are testable without a database and
//! a store query cannot express them differently.
//!
//! # Why eligibility is a separate stage, and why it comes first
//!
//! The document's order is not a preference. Eligibility is a **filter**, and its six rules include
//! properties that no amount of ranking can repair:
//!
//! - **actor authorization** and **workspace policy** â€” a memory from another workspace is not a
//!   lower-ranked result, it is not a result. Ranking it and then dropping it would make the ranking
//!   depend on how many foreign memories happened to be in the candidate set.
//! - **status/validity/retention** â€” a superseded or expired claim is not "less relevant", it is not
//!   current truth, and the document requires it stop being retrieved *as current*. A ranker that could
//!   surface it would need every consumer to re-check.
//! - **sensitivity and destination policy** â€” the one rule that is about the *destination* rather than the
//!   memory. It is a filter because admitting a restricted claim and then redacting it is the failure the
//!   rule exists to prevent: a redaction is a transformation of content already inside the assembly step.
//! - **memory type allowed for the use case** and **source trust minimum** â€” both are stated by the caller
//!   as requirements. A caller that asked for a minimum trust and received something below it would have to
//!   re-filter, and the second filter is where one of the two is forgotten.
//!
//! # Why the ranking stores its components
//!
//! The document: "Combine independently inspectable signals â€¦ No single signal may dominate by accident.
//! **Store component scores and the final inclusion reason.**" So a [`ScoredMemory`] carries every signal's
//! own score, and the reason is derived from them rather than computed separately.
//!
//! Three properties make that more than bookkeeping, and each is a test:
//!
//! 1. **No single signal can carry a memory on its own.** Every weight is at most a quarter of the total, so
//!    the strongest possible contribution from one signal is below what a ranker needs to place a memory at
//!    the top of the set by itself. See [`SIGNAL_WEIGHTS`].
//! 2. **The arithmetic is integer, not floating point.** A float score would make the ranking depend on
//!    platform rounding, and two builds could order the same memories differently â€” which makes a stored
//!    score unreproducible and an inclusion reason unverifiable.
//! 3. **The scores are additive and independent.** A signal is computed from the record and the query only,
//!    so removing one signal changes one component and the total by a known amount rather than re-weighting
//!    the others.
//!
//! # What this stage deliberately does not do
//!
//! **Active project/task relevance is absent**: there is no active project or task in the record, so a score
//! for it would be a constant dressed as a signal.
//!
//! **Relationship overlap is absent.** The document lists "entity/relationship overlap", and a relation
//! graph exists in storage (`entity_relations`) but nothing traverses it yet, so only direct entity overlap
//! is scored. `P4-004`'s storage half owns the read; traversing relations is a graph walk with its own cost
//! and cycle question, and its own slice.
//!
//! **Semantic similarity is present but conditional**, which is the difference between this module and the
//! previous slice. `P4-005` built the embedding port, so the signal now exists — but it needs a query vector
//! and a memory vector, and neither is in the record or the query. They arrive as a [`ScoringContext`], and
//! when it carries no index the signal is zero rather than absent from the table. The distinction matters:
//! the row is in [`SIGNAL_WEIGHTS`] because the *signal* is implemented, and a caller can see from
//! [`SemanticAbsence`] that the reason it produced nothing was missing data rather than a real zero.
//!
//! # An embedding is an index, not a truth
//!
//! Two constraints keep it that way, and both are structural rather than advisory:
//!
//! 1. **A memory is fully usable without one.** Nothing here requires an embedding to store, retrieve, or
//!    rank a memory — the semantic term is one row of nine and a memory with no vector still scores on the
//!    other eight.
//! 2. **A vector from a different model is not comparable, and this module refuses rather than approximates.**
//!    [`ScoringContext::semantic_outcome`] compares provider, model, and version before any arithmetic,
//!    because a cosine over two vectors from different models is a number with no meaning that still lands
//!    in the usual range — so a missing check does not look like a bug, it looks like a slightly worse
//!    ranking.
//!
//! # Scoring is not selection
//!
//! [`rank`] scores and orders; it does **not** apply budgets or diversification, because those need the
//! whole candidate set and are about the *result* rather than about a memory. They live in
//! [`diversify`], which is a separate function for exactly that reason.

use crate::id::{EntityId, MemoryId, WorkspaceId};
use crate::memory::{
    EffectiveMemoryStatus, EntityRef, MemoryRecord, MemorySourceKind, MemoryTrust, MemoryType,
};
use crate::sensitivity::Sensitivity;
use crate::timestamp::UtcTimestamp;

/// Why a memory was not eligible for a query.
///
/// One variant per rule the document lists, so a caller can report which rule excluded a memory â€” "the
/// destination may not receive it" and "it is expired" are different answers to "why is this not being
/// used", which the document requires be answerable from stored data rather than composed afterwards.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ineligibility {
    /// The memory belongs to another workspace.
    ///
    /// An **equality**, not a containment or a hierarchy: the document's own boundary, and
    /// [`MemoryRecord::is_visible_in`] is the single place it is asked.
    ForeignWorkspace,
    /// The claim is not current truth at the query instant: superseded, expired, or deleted.
    ///
    /// The document requires obsolete claims "stop being retrieved as current truth", so a superseded
    /// memory is excluded rather than down-ranked â€” the correction trail is retained, not the ranking.
    NotCurrent {
        /// The status the memory actually has.
        status: EffectiveMemoryStatus,
    },
    /// The memory's level is above what the destination may receive.
    AboveDestination {
        /// The memory's own level.
        sensitivity: Sensitivity,
        /// The ceiling the destination may receive.
        destination: Sensitivity,
    },
    /// The memory's type is not one the use case allows.
    TypeNotAllowed {
        /// The memory's type.
        memory_type: MemoryType,
    },
    /// The memory's source trust is below the query's minimum.
    TrustBelowMinimum {
        /// The source's trust class.
        trust: MemoryTrust,
        /// The minimum the query requires.
        minimum: MemoryTrust,
    },
    /// The memory's confidence is below the query's minimum.
    ///
    /// Not one of the document's six rows, and added because the two rules are genuinely different: trust
    /// is what the *origin* can be asked about and confidence is how well *this* claim is supported. A
    /// caller assembling context for a statement of fact needs the second, and the document's own
    /// confidence vocabulary exists to be asked. Recorded here rather than folded into the trust rule so a
    /// reader can see which one excluded a memory.
    ConfidenceBelowMinimum,
}

impl Ineligibility {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ForeignWorkspace => "foreign_workspace",
            Self::NotCurrent { .. } => "not_current",
            Self::AboveDestination { .. } => "above_destination",
            Self::TypeNotAllowed { .. } => "type_not_allowed",
            Self::TrustBelowMinimum { .. } => "trust_below_minimum",
            Self::ConfidenceBelowMinimum => "confidence_below_minimum",
        }
    }
}

/// What a caller is retrieving memory for.
///
/// # Why the requirements are typed rather than booleans
///
/// Every field is a **statement about the use case**, and each is the input to exactly one eligibility
/// rule. A boolean (`require_trusted: true`) would lose which trust a caller meant, and the default for a
/// missing field would be the permissive one â€” which is how a filter becomes a formality.
///
/// `destination` is the **most sensitive content the destination may receive**, following
/// `jarvis_core::assemble_context`: `Sensitivity::Restricted` as a ceiling means a local destination that
/// accepts anything, while `Internal` means a third-party model that may not receive confidential content.
/// Phrasing it as a ceiling is what makes [`Sensitivity::can_flow_to`] the entire check, so a caller cannot
/// widen what is permitted by relabelling the destination.
#[derive(Clone, Debug)]
pub struct MemoryQuery {
    /// The workspace to search, which is also the retrieval boundary.
    pub workspace_id: WorkspaceId,
    /// The destination's sensitivity ceiling.
    pub destination: Sensitivity,
    /// The free text to match, for the keyword signal. Empty means "no text signal".
    pub text: String,
    /// The entities the question is about, for the exact-identifier and overlap signals.
    pub entity_ids: Vec<EntityId>,
    /// The memory types the use case allows. Empty means "any".
    pub allowed_types: Vec<MemoryType>,
    /// The lowest source trust the use case accepts.
    pub minimum_trust: MemoryTrust,
    /// The lowest confidence the use case accepts as a statement of fact.
    pub minimum_confidence: crate::memory::MemoryConfidence,
    /// The instant to evaluate validity at, which is a parameter rather than the clock.
    pub at: UtcTimestamp,
}

impl MemoryQuery {
    /// Builds a query with the strictest defaults: local-only destination, no type restriction, and
    /// authoritative sources at confirfmed confidence.
    ///
    /// # Why the defaults are strict
    ///
    /// A default is what a caller who did not think about a field gets, and for a retrieval the permissive
    /// default is a disclosure. `Sensitivity::Internal` as the destination ceiling means a third-party
    /// model receives nothing confidential until a caller says otherwise, and `Authoritative` as the trust
    /// minimum means an inference or a web page is not offered as an answer until a caller asks for it.
    #[must_use]
    pub fn new(workspace_id: WorkspaceId, at: UtcTimestamp) -> Self {
        Self {
            workspace_id,
            destination: Sensitivity::Internal,
            text: String::new(),
            entity_ids: Vec::new(),
            allowed_types: Vec::new(),
            minimum_trust: MemoryTrust::Authoritative,
            minimum_confidence: crate::memory::MemoryConfidence::Confirmed,
            at,
        }
    }

    /// Sets the free text to match.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Sets the entities the question is about.
    #[must_use]
    pub fn with_entities(mut self, entity_ids: Vec<EntityId>) -> Self {
        self.entity_ids = entity_ids;
        self
    }

    /// Sets the destination ceiling.
    #[must_use]
    pub const fn with_destination(mut self, destination: Sensitivity) -> Self {
        self.destination = destination;
        self
    }

    /// Sets the required minimum source trust.
    #[must_use]
    pub const fn with_minimum_trust(mut self, minimum: MemoryTrust) -> Self {
        self.minimum_trust = minimum;
        self
    }

    /// Sets the required minimum confidence.
    #[must_use]
    pub const fn with_minimum_confidence(
        mut self,
        minimum: crate::memory::MemoryConfidence,
    ) -> Self {
        self.minimum_confidence = minimum;
        self
    }

    /// Sets the allowed memory types.
    #[must_use]
    pub fn with_allowed_types(mut self, types: Vec<MemoryType>) -> Self {
        self.allowed_types = types;
        self
    }

    /// Applies the six eligibility rules and reports the first that refuses.
    ///
    /// # Errors
    ///
    /// Returns the [`Ineligibility`] naming the rule that excluded the memory. The **order** is the
    /// document's, with one exception recorded below, and it is chosen so the reported reason is the most
    /// specific: the workspace first (a foreign memory is not a ranking question at all), then the
    /// destination (a disclosure is the most severe outcome), then currency, then the three caller-stated
    /// requirements.
    ///
    /// The exception is the workspace check, which the document lists as "actor authorization" first and
    /// "workspace and sharing policy" second. They are one check here because this platform's actor
    /// authorization is workspace membership: `P4-002`'s reads all bind `workspace_id` as their scope, and a
    /// separate actor rule with no second workspace to distinguish would be a rule that always passes.
    pub fn is_eligible(&self, record: &MemoryRecord) -> Result<(), Ineligibility> {
        if !record.is_visible_in(self.workspace_id) {
            return Err(Ineligibility::ForeignWorkspace);
        }
        if !record.sensitivity().can_flow_to(self.destination) {
            return Err(Ineligibility::AboveDestination {
                sensitivity: record.sensitivity(),
                destination: self.destination,
            });
        }
        let status = record.effective_status_at(self.at);
        if !status.is_current_truth() {
            return Err(Ineligibility::NotCurrent { status });
        }
        if !self.allowed_types.is_empty() && !self.allowed_types.contains(&record.memory_type()) {
            return Err(Ineligibility::TypeNotAllowed {
                memory_type: record.memory_type(),
            });
        }
        if record.source().trust().level() < self.minimum_trust.level() {
            return Err(Ineligibility::TrustBelowMinimum {
                trust: record.source().trust(),
                minimum: self.minimum_trust,
            });
        }
        if record.confidence().level() < self.minimum_confidence.level() {
            return Err(Ineligibility::ConfidenceBelowMinimum);
        }
        Ok(())
    }
}

/// One signal's score, on a fixed 0..=[`SIGNAL_SCALE`] range.
pub type SignalScore = u16;

/// The top of a single signal's range, before weighting.
pub const SIGNAL_SCALE: SignalScore = 1000;

/// The weight of each signal, out of [`TOTAL_WEIGHT`].
///
/// # Why a table, and why every weight is bounded
///
/// "No single signal may dominate by accident" is only enforceable if the weights are one visible set that
/// a test can check. Three properties hold, and each has its own assertion:
///
/// 1. **The weights sum to [`TOTAL_WEIGHT`]** (`1000`), so a total score has a known maximum and a
///    threshold means the same thing at every call site.
/// 2. **No weight exceeds [`MAX_SIGNAL_WEIGHT`]**, a quarter of the total. So the highest a single signal
///    can contribute is a quarter of the maximum â€” a memory that scores perfectly on one signal and zero on
///    every other cannot reach the top quarter of the range, and therefore cannot displace a memory that is
///    good on several. That is the property "may not dominate **by accident**" made arithmetic.
/// 3. **Every signal here is computable, and one of them is conditional.** Eight of the nine are computed
///    from the record and the query alone. Semantic similarity needs embeddings, which `signals_for`
///    receives as an optional index — see [`SemanticSimilarity`]. When no index is supplied that signal is
///    zero, and the row is present because the *signal* exists rather than because the data does.
///
/// The order is the document's own list, minus the two signals recorded as absent in the module doc.
/// Semantic similarity sits third because that is where the document lists it, above entity overlap.
pub const SIGNAL_WEIGHTS: SignalWeights = SignalWeights {
    // Exact identifiers and aliases: the entity the question names is the entity the memory is about.
    exact_identifier: 250,
    // Full-text/keyword match against the memory's content.
    keyword: 175,
    // Semantic similarity: what the memory means, against what the question means.
    semantic: 150,
    // Entity overlap: some, but not all, of the question's entities appear.
    entity_overlap: 125,
    // Recency: how recently the memory was created or last used.
    recency: 100,
    // Temporal relevance: whether the query instant falls inside the claim's validity window.
    temporal: 75,
    // The rank the memory itself declares.
    importance: 60,
    // What the origin can be asked about.
    source_reliability: 40,
    // How often the memory has been usefully retrieved before.
    reinforcement: 25,
};

/// The weights of every signal, as one comparable value.
///
/// A named struct rather than an array so a weight cannot be read by position â€” the array form would make
/// `weights[3]` compile while naming a different signal than the reader thinks, which is the transposition
/// hazard the repository's other construction sites already group their parameters to avoid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalWeights {
    /// Weight of the exact-identifier signal.
    pub exact_identifier: u16,
    /// Weight of the keyword signal.
    pub keyword: u16,
    /// Weight of the semantic-similarity signal.
    pub semantic: u16,
    /// Weight of the entity-overlap signal.
    pub entity_overlap: u16,
    /// Weight of the recency signal.
    pub recency: u16,
    /// Weight of the temporal signal.
    pub temporal: u16,
    /// Weight of the importance signal.
    pub importance: u16,
    /// Weight of the source-reliability signal.
    pub source_reliability: u16,
    /// Weight of the reinforcement signal.
    pub reinforcement: u16,
}

/// The total of [`SIGNAL_WEIGHTS`], and therefore the maximum a total score can reach.
pub const TOTAL_WEIGHT: u32 = 1000;

/// The largest weight any single signal may carry.
pub const MAX_SIGNAL_WEIGHT: u16 = 250;

/// The number of signals, so a new signal cannot be added to the struct and forgotten in the array forms.
pub const SIGNAL_COUNT: usize = 9;

impl SignalWeights {
    /// Returns every weight, for a test or an operator display.
    #[must_use]
    pub const fn all(&self) -> [(&'static str, u16); SIGNAL_COUNT] {
        [
            ("exact_identifier", self.exact_identifier),
            ("keyword", self.keyword),
            ("semantic", self.semantic),
            ("entity_overlap", self.entity_overlap),
            ("recency", self.recency),
            ("temporal", self.temporal),
            ("importance", self.importance),
            ("source_reliability", self.source_reliability),
            ("reinforcement", self.reinforcement),
        ]
    }

    /// Returns the sum of every weight.
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.exact_identifier as u32
            + self.keyword as u32
            + self.semantic as u32
            + self.entity_overlap as u32
            + self.recency as u32
            + self.temporal as u32
            + self.importance as u32
            + self.source_reliability as u32
            + self.reinforcement as u32
    }
}

/// Every signal's own score for one memory, before weighting.
///
/// Stored rather than discarded, because the document requires the component scores be kept: they are what
/// makes a ranking auditable, and what lets an operator answer "why was this retrieved" from stored data
/// rather than from a reconstruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemorySignals {
    /// How completely the question's entities are the memory's entities.
    pub exact_identifier: SignalScore,
    /// How much of the question's text appears in the memory.
    pub keyword: SignalScore,
    /// How close the memory's meaning is to the question's, when both have a comparable embedding.
    pub semantic: SignalScore,
    /// How much of the question's entity set the memory shares at all.
    pub entity_overlap: SignalScore,
    /// How recent the memory is, relative to the query instant.
    pub recency: SignalScore,
    /// Whether the claim is inside its validity window at the query instant.
    pub temporal: SignalScore,
    /// The importance the memory declares.
    pub importance: SignalScore,
    /// What the memory's origin can be asked about, scaled to the signal range.
    pub source_reliability: SignalScore,
    /// How often the memory has been usefully retrieved.
    pub reinforcement: SignalScore,
}

impl MemorySignals {
    /// Returns every signal with its name, for a test or an operator display.
    #[must_use]
    pub const fn all(&self) -> [(&'static str, SignalScore); SIGNAL_COUNT] {
        [
            ("exact_identifier", self.exact_identifier),
            ("keyword", self.keyword),
            ("semantic", self.semantic),
            ("entity_overlap", self.entity_overlap),
            ("recency", self.recency),
            ("temporal", self.temporal),
            ("importance", self.importance),
            ("source_reliability", self.source_reliability),
            ("reinforcement", self.reinforcement),
        ]
    }

    /// Returns the weighted total, out of [`TOTAL_WEIGHT`].
    ///
    /// # Why this sums the per-signal contributions rather than scaling the whole sum once
    ///
    /// Integer division is not distributive: `(a*w + b*w) / S` and `a*w / S + b*w / S` differ by up to one
    /// per term, because each truncates its own remainder. Scaling once would make the total disagree with
    /// the sum of the displayed contributions — so an operator shown the component scores would be reading
    /// arithmetic that does not add up, which is exactly what an explanation must not be. Summing the
    /// contributions makes the explanation **the** calculation, and the test that asserts they agree is
    /// asserting the property rather than a coincidence of the values.
    #[must_use]
    pub const fn total(&self, weights: &SignalWeights) -> u32 {
        let contributions = self.contributions(weights);
        contributions[0].1
            + contributions[1].1
            + contributions[2].1
            + contributions[3].1
            + contributions[4].1
            + contributions[5].1
            + contributions[6].1
            + contributions[7].1
            + contributions[8].1
    }

    /// Returns the weighted contribution of each signal, for an explanation.
    #[must_use]
    pub const fn contributions(
        &self,
        weights: &SignalWeights,
    ) -> [(&'static str, u32); SIGNAL_COUNT] {
        [
            (
                "exact_identifier",
                self.exact_identifier as u32 * weights.exact_identifier as u32
                    / SIGNAL_SCALE as u32,
            ),
            (
                "keyword",
                self.keyword as u32 * weights.keyword as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "semantic",
                self.semantic as u32 * weights.semantic as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "entity_overlap",
                self.entity_overlap as u32 * weights.entity_overlap as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "recency",
                self.recency as u32 * weights.recency as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "temporal",
                self.temporal as u32 * weights.temporal as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "importance",
                self.importance as u32 * weights.importance as u32 / SIGNAL_SCALE as u32,
            ),
            (
                "source_reliability",
                self.source_reliability as u32 * weights.source_reliability as u32
                    / SIGNAL_SCALE as u32,
            ),
            (
                "reinforcement",
                self.reinforcement as u32 * weights.reinforcement as u32 / SIGNAL_SCALE as u32,
            ),
        ]
    }
}

/// Why a memory was selected, in terms a user can be told.
///
/// # Why this is derived rather than computed alongside
///
/// The document requires "the final inclusion reason" be stored, and the risk of computing it separately
/// from the scores is that the two can disagree â€” a memory reported as found by keyword while its keyword
/// signal is zero. Deriving the reason **from the components** makes that unrepresentable, and it is what
/// makes the reason verifiable: a test can assert a reason against the contribution table rather than
/// against a second implementation of the same rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionReason {
    /// The question named an entity the memory is about.
    ExactIdentifier,
    /// The question's text appears in the memory.
    KeywordMatch,
    /// The memory shares some of the question's entities.
    EntityOverlap,
    /// The memory means something close to what the question asked, without sharing its words.
    SemanticSimilarity,
    /// The claim is inside its validity window and recent, with nothing matching above.
    ///
    /// A memory included for this reason did not match the question; it is offered because it is current
    /// and near. That is a legitimate result for a question like "what is relevant now", and naming it
    /// separately is what stops such a memory being reported as a match.
    RecentAndCurrent,
    /// The memory declares a high importance, with nothing above it.
    Important,
}

impl SelectionReason {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ExactIdentifier => "exact_identifier",
            Self::KeywordMatch => "keyword_match",
            Self::EntityOverlap => "entity_overlap",
            Self::SemanticSimilarity => "semantic_similarity",
            Self::RecentAndCurrent => "recent_and_current",
            Self::Important => "important",
        }
    }

    /// Returns whether this reason means the memory actually matched the question.
    ///
    /// The distinction a caller needs for phrasing: a matched memory can be presented as an answer, while a
    /// recent-and-current one is context. Collapsing them would let a memory that answered nothing be
    /// stated as if it did.
    #[must_use]
    pub const fn is_a_match(&self) -> bool {
        matches!(
            self,
            Self::ExactIdentifier
                | Self::KeywordMatch
                | Self::EntityOverlap
                | Self::SemanticSimilarity
        )
    }
}

/// One memory with its components, total, and reason.
#[derive(Clone, Debug)]
pub struct ScoredMemory {
    record: MemoryRecord,
    signals: MemorySignals,
    total: u32,
    reason: SelectionReason,
}

impl ScoredMemory {
    /// Returns the memory.
    #[must_use]
    pub const fn record(&self) -> &MemoryRecord {
        &self.record
    }

    /// Returns every signal's own score.
    #[must_use]
    pub const fn signals(&self) -> &MemorySignals {
        &self.signals
    }

    /// Returns the weighted total, out of [`TOTAL_WEIGHT`].
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.total
    }

    /// Returns why this memory was selected.
    #[must_use]
    pub const fn reason(&self) -> SelectionReason {
        self.reason
    }

    /// Consumes the value, returning the memory.
    #[must_use]
    pub fn into_record(self) -> MemoryRecord {
        self.record
    }
}

/// Scores one memory against a query. Eligibility is the caller's to have checked.
///
/// # Why this does not check eligibility
///
/// [`rank`] filters first and then scores, and a scorer that also filtered would compute a total for a
/// memory it then discards â€” and, worse, would have to decide what to *return* for an ineligible one. The
/// split means a caller that has a memory and a query can score it for a display without re-running the
/// filter, and there is one place that decides who is eligible.
#[must_use]
pub fn score(
    record: &MemoryRecord,
    query: &MemoryQuery,
    context: &ScoringContext<'_>,
    weights: &SignalWeights,
) -> ScoredMemory {
    let signals = signals_for(record, query, context);
    let total = signals.total(weights);
    let reason = reason_for(&signals, weights);
    ScoredMemory {
        record: record.clone(),
        signals,
        total,
        reason,
    }
}

/// Ranks every eligible memory, best first, dropping the ineligible.
///
/// # Why the drops are reported rather than silent
///
/// The document requires an answer to "why was something not used", and an ineligible memory that simply
/// vanished leaves that question unanswerable. So the result carries the refusal and the rule that
/// produced it, and a caller can log or display it.
///
/// Ties are broken by memory identifier, so the order of two equally-scored memories is **stable across
/// runs**. Without it the order would depend on the candidate set's arrival order, which for a
/// database-backed read is a query plan away from changing between builds.
#[must_use]
pub fn rank(
    records: &[MemoryRecord],
    query: &MemoryQuery,
    context: &ScoringContext<'_>,
    weights: &SignalWeights,
) -> MemorySelection {
    let mut excluded = Vec::new();
    let mut scored = Vec::new();
    for record in records {
        match query.is_eligible(record) {
            Ok(()) => scored.push(score(record, query, context, weights)),
            Err(reason) => excluded.push(ExcludedMemory {
                memory_id: record.id(),
                reason,
            }),
        }
    }
    // Sorted by total descending, then by identifier ascending, so the order is total and stable.
    scored.sort_by(|left, right| {
        right
            .total
            .cmp(&left.total)
            .then_with(|| left.record.id().cmp(&right.record.id()))
    });
    MemorySelection {
        scored,
        excluded,
        dropped: Vec::new(),
    }
}

/// The ranked result, everything ineligible, and everything diversification left out.
///
/// Three lists rather than one with a status field, because the three answers are acted on differently: a
/// selected memory is used, an ineligible one is impossible (and reports the rule), and a dropped one is
/// available if the caller wants it. A single list would make every reader filter, and the filter is where
/// a caller would forget that a dropped memory is not a refused one.
#[derive(Clone, Debug)]
pub struct MemorySelection {
    scored: Vec<ScoredMemory>,
    excluded: Vec<ExcludedMemory>,
    dropped: Vec<DroppedMemory>,
}

impl MemorySelection {
    /// Returns the ranked memories, best first.
    #[must_use]
    pub fn scored(&self) -> &[ScoredMemory] {
        &self.scored
    }

    /// Returns every memory that was not eligible, with the rule that excluded it.
    #[must_use]
    pub fn excluded(&self) -> &[ExcludedMemory] {
        &self.excluded
    }

    /// Returns every eligible memory that diversification left out, with the cap that dropped it.
    ///
    /// Empty until [`diversify`] runs, which is the honest state: nothing has been capped yet.
    #[must_use]
    pub fn dropped(&self) -> &[DroppedMemory] {
        &self.dropped
    }

    /// Returns whether nothing was selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scored.is_empty()
    }

    /// Returns the number of memories scored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scored.len()
    }
}

/// A memory that was not eligible, and the rule that refused it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExcludedMemory {
    /// The memory that was excluded.
    pub memory_id: crate::id::MemoryId,
    /// The rule that excluded it.
    pub reason: Ineligibility,
}

// ------------------------------------------------------------------------------------------------
// Diversification and budgets
// ------------------------------------------------------------------------------------------------

/// Why a memory was dropped after being scored.
///
/// # Why this is not an [`Ineligibility`]
///
/// The two answer different questions and are fixed differently. An ineligible memory **may not** be
/// retrieved — the destination cannot receive it, or it is not current — while a memory dropped here was
/// perfectly eligible and was left out because the result already holds enough of its kind. Merging them
/// would report "you have seen enough preferences" as "this preference is not available to you", and an
/// operator acting on the first would go looking for a policy problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiversityReason {
    /// The result already holds the budget's maximum for this memory type.
    TypeBudget {
        /// The type that reached its cap.
        memory_type: MemoryType,
    },
    /// The result already holds the budget's maximum for one of the memory's entities.
    EntityBudget {
        /// The entity that reached its cap.
        entity_id: EntityId,
    },
    /// The result already holds the budget's maximum for this source kind.
    SourceKindBudget {
        /// The source kind that reached its cap.
        source_kind: MemorySourceKind,
    },
    /// The result reached its total cap.
    TotalBudget,
}

impl DiversityReason {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TypeBudget { .. } => "type_budget",
            Self::EntityBudget { .. } => "entity_budget",
            Self::SourceKindBudget { .. } => "source_kind_budget",
            Self::TotalBudget => "total_budget",
        }
    }
}

/// A memory dropped by diversification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DroppedMemory {
    /// The memory that was dropped.
    pub memory_id: crate::id::MemoryId,
    /// Why it was dropped.
    pub reason: DiversityReason,
}

/// The caps a result must respect.
///
/// # Why every cap is explicit and bounded
///
/// The document requires "per-source/per-category budgets" and that results "diversif[y] across memory
/// types/entities". Those are the same mechanism: a cap per category is what forces diversity, because a
/// result that filled its allowance with preferences has no room for a preference and therefore carries
/// something else. So each cap is a field rather than a hidden constant, and [`Self::new`] refuses a cap of
/// zero — a zero cap would drop every memory of that category and read as "the category is unavailable".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiversityBudget {
    /// The most memories of any one type.
    pub max_per_type: usize,
    /// The most memories sharing any one entity.
    pub max_per_entity: usize,
    /// The most memories from any one source kind.
    ///
    /// The document's "per-source" budget. It is separate from the type cap because the two bound different
    /// concentrations: a result could be diverse across types while every claim in it came from one
    /// provider, which is a single point of failure presented as consensus.
    pub max_per_source_kind: usize,
    /// The most memories in the result overall.
    pub max_total: usize,
}

/// The default caps, chosen to be small enough to force diversity on a realistic candidate set.
pub const DEFAULT_DIVERSITY_BUDGET: DiversityBudget = DiversityBudget {
    max_per_type: 4,
    max_per_entity: 3,
    max_per_source_kind: 6,
    max_total: 12,
};

impl DiversityBudget {
    /// Builds a budget, refusing any cap of zero.
    ///
    /// # Errors
    ///
    /// Returns the name of the field that was zero. A zero cap is refused rather than treated as "unlimited"
    /// or as "none": both readings are plausible and they are opposites, so a caller that passed one would
    /// get behaviour it did not choose.
    pub fn new(
        max_per_type: usize,
        max_per_entity: usize,
        max_per_source_kind: usize,
        max_total: usize,
    ) -> Result<Self, &'static str> {
        let budget = Self {
            max_per_type,
            max_per_entity,
            max_per_source_kind,
            max_total,
        };
        if max_per_type == 0 {
            return Err("max_per_type");
        }
        if max_per_entity == 0 {
            return Err("max_per_entity");
        }
        if max_per_source_kind == 0 {
            return Err("max_per_source_kind");
        }
        if max_total == 0 {
            return Err("max_total");
        }
        Ok(budget)
    }
}

/// Applies the caps to a ranked result, keeping the best of each category.
///
/// # Why the caps are applied in rank order rather than by category weight
///
/// A memory's position is already the combined judgment of every signal, so walking the ranked list and
/// keeping a memory whenever its categories have room is the only order that respects it. Selecting by
/// category first — "take the best preference, then the best fact" — would override the ranking with a
/// category order the scoring never expressed, and the document's own rule is that no single signal may
/// dominate: a category order imposed afterwards is a signal by another name.
///
/// # Why a multi-entity memory counts against all of its entities
///
/// A memory linked to two saturated entities is dropped, which is the conservative direction. The
/// alternative — counting only its first entity — would let a memory about a saturated person in through
/// another of its entities, and the cap exists to bound how much of the result is about one person.
#[must_use]
pub fn diversify(selection: MemorySelection, budget: &DiversityBudget) -> MemorySelection {
    let mut kept = Vec::new();
    let mut dropped = selection.dropped.clone();
    // `HashMap` rather than `BTreeMap`: the three vocabularies derive `Hash` and `Eq` and not `Ord`, which
    // is the right set for a value that is compared and not ordered — and nothing here iterates a map, so
    // the iteration order a `BTreeMap` would provide is not needed.
    let mut per_type: std::collections::HashMap<MemoryType, usize> =
        std::collections::HashMap::new();
    let mut per_entity: std::collections::HashMap<EntityId, usize> =
        std::collections::HashMap::new();
    let mut per_source: std::collections::HashMap<MemorySourceKind, usize> =
        std::collections::HashMap::new();

    for scored in selection.scored {
        if kept.len() >= budget.max_total {
            dropped.push(DroppedMemory {
                memory_id: scored.record.id(),
                reason: DiversityReason::TotalBudget,
            });
            continue;
        }
        let memory_type = scored.record.memory_type();
        if per_type.get(&memory_type).copied().unwrap_or(0) >= budget.max_per_type {
            dropped.push(DroppedMemory {
                memory_id: scored.record.id(),
                reason: DiversityReason::TypeBudget { memory_type },
            });
            continue;
        }
        let source_kind = scored.record.source().kind();
        if per_source.get(&source_kind).copied().unwrap_or(0) >= budget.max_per_source_kind {
            dropped.push(DroppedMemory {
                memory_id: scored.record.id(),
                reason: DiversityReason::SourceKindBudget { source_kind },
            });
            continue;
        }
        // Any entity at its cap drops the memory, and the *first* such entity is the one reported, so the
        // reason names a specific entity rather than "one of them" — an operator reading it can go and look.
        let saturated = scored
            .record
            .entities()
            .iter()
            .map(EntityRef::entity_id)
            .find(|entity_id| {
                per_entity.get(entity_id).copied().unwrap_or(0) >= budget.max_per_entity
            });
        if let Some(entity_id) = saturated {
            dropped.push(DroppedMemory {
                memory_id: scored.record.id(),
                reason: DiversityReason::EntityBudget { entity_id },
            });
            continue;
        }

        // Admitted: every one of its categories is charged, which is what makes the caps hold.
        *per_type.entry(memory_type).or_insert(0) += 1;
        *per_source.entry(source_kind).or_insert(0) += 1;
        for entity_id in scored.record.entities().iter().map(EntityRef::entity_id) {
            *per_entity.entry(entity_id).or_insert(0) += 1;
        }
        kept.push(scored);
    }

    MemorySelection {
        scored: kept,
        excluded: selection.excluded,
        dropped,
    }
}

// ------------------------------------------------------------------------------------------------
// The signals
// ------------------------------------------------------------------------------------------------

/// Scales a numerator and denominator to a signal score, keeping the fraction's precision.
///
/// # Why this is one function rather than a cast at each site
///
/// Every signal is a fraction of a bound, and the obvious `numerator / denominator` in integers is **zero**
/// for every partial case: two of three words truncates to nothing, so a memory matching most of a question
/// would score as if it matched none. So the numerator is scaled by [`SIGNAL_SCALE`] before the division,
/// and doing that at each of eight sites is eight chances to forget the multiply or to divide first.
///
/// The conversions are explicit rather than `as` casts because each one is a real question: a denominator of
/// zero has no score, and a count larger than `u32` cannot occur for content the domain bounds. `u128` holds
/// the scaled numerator for the largest denominator any bound here permits.
fn scaled(numerator: usize, denominator: usize) -> SignalScore {
    if denominator == 0 {
        return 0;
    }
    let scaled = (numerator as u128) * u128::from(SIGNAL_SCALE) / (denominator as u128);
    // The result cannot exceed `SIGNAL_SCALE` because the numerator is at most the denominator, and it is
    // written as a clamp rather than a narrowing cast so an impossible value is visible rather than silent.
    u16::try_from(scaled)
        .unwrap_or(SIGNAL_SCALE)
        .min(SIGNAL_SCALE)
}

/// The query's semantic side and the per-memory vectors a ranking may compare it against.
///
/// # Why this is one value rather than a second argument in two places
///
/// Semantic similarity is the only signal that needs something the record and the query do not carry, and
/// the thing it needs comes in **two halves that must agree**: the query's vector and the memory's. Passing
/// the query vector separately from the memory lookup would make "a query vector with no memory vectors"
/// and "memory vectors with no query vector" both representable, and both are meaningless — so they are one
/// optional value, and `None` means "no embedding index for this ranking".
///
/// # Why the vectors are borrowed rather than owned
///
/// A candidate set is scored once and the vectors are the largest values involved. Borrowing lets a caller
/// hold one decoded index across a whole ranking, so a hundred memories do not become a hundred clones of
/// their vectors. What the *result* retains is the metadata needed to explain the score, not the floats.
///
/// # Why there is no `Default`
///
/// `ScoringContext::default()` would read as "an unspecified context" and quietly disable a signal, which is
/// the shape `P4-004` refused for the query defaults: a default is what a caller who did not think about the
/// field gets, and for a *scoring input* the consequence of not thinking is a silently missing signal. A
/// caller who wants no index writes [`ScoringContext::without_semantics`], which says so.
///
/// # Why the lookup is owned rather than borrowed
///
/// It was a `&dyn Fn` first, and every call site that passed a closure literal failed to compile: a
/// temporary closure borrowed into a longer-lived context is dropped at the end of the statement. That
/// error is correct but it is the wrong *shape* for a caller — the natural way to write a lookup is inline,
/// and requiring a `let` binding for it first is a papercut on every use. Owning a box makes the
/// one-allocation cost a per-ranking cost rather than a per-memory one, which is the trade the callers
/// would make anyway.
pub struct ScoringContext<'a> {
    semantic: Option<SemanticScoring<'a>>,
}

/// The two halves of a semantic comparison, which must be present together.
struct SemanticScoring<'a> {
    query: SemanticVector<'a>,
    vectors: SemanticLookup<'a>,
}

/// Looks up the vector stored for one memory.
///
/// Written as an alias rather than inline because the type is a boxed closure over a lifetime, which is
/// noisy enough at the field, the constructor, and the call site that naming it is what keeps the three
/// readable — and it makes the allocation decision visible in one place.
type SemanticLookup<'a> = Box<dyn Fn(&MemoryId) -> Option<SemanticVector<'a>> + 'a>;

/// A vector with the identity a comparison depends on.
///
/// # Why the provider, model, and version are here and not inferred
///
/// "Never compare vectors with incompatible metadata" is a rule about a pair, so both sides must carry the
/// identity. A vector without it cannot be compared by any code that takes the rule seriously, and
/// `jarvis-models`' `EmbeddingMetadata::ensure_comparable_with` is what performs the comparison — this type
/// is the domain-side carrier for the four fields that rule needs.
///
/// # Why the dimension is not here
///
/// The vector's own length is the dimension, and a separate field could contradict it. `jarvis-models`
/// checks the two against each other at construction; repeating the field here would let a caller construct
/// a pair this module believes and `jarvis-models` would refuse.
#[derive(Clone, Copy)]
pub struct SemanticVector<'a> {
    /// The embedding provider the vector came from.
    pub provider: &'a str,
    /// The embedding model.
    pub model: &'a str,
    /// The model version, when the provider states one.
    pub version: Option<&'a str>,
    /// Whether the vector's length is one.
    pub normalized: SemanticNormalization,
    /// The vector's components.
    pub values: &'a [f32],
}

/// Whether a provider normalizes the vectors it returns.
///
/// `Unknown` is the default for the same reason `jarvis-models` defaults to it: the dot-product shortcut is
/// the permissive assumption on a distance, and the one that goes wrong quietly. Here it makes the cosine
/// division happen, so an unknown normalization is *slower and correct* rather than *faster and wrong*.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SemanticNormalization {
    /// The provider states the vectors are length one.
    Normalized,
    /// The provider states the vectors are not normalized.
    Unnormalized,
    /// The provider states nothing.
    #[default]
    Unknown,
}

/// Why a memory's semantic signal is zero.
///
/// Recorded rather than folded into "no score", because the two are different answers: a memory with no
/// embedding was never a candidate for this signal, while a memory whose embedding is *incomparable* is one.
/// An operator asking "why did semantic similarity not help here" needs to tell the two apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticAbsence {
    /// The ranking has no embedding index at all.
    NoIndex,
    /// The query has no embedding.
    NoQueryVector,
    /// The memory has no embedding.
    NoMemoryVector,
    /// The memory's embedding is not comparable with the query's, and the field that differs is named.
    Incomparable(&'static str),
    /// Both vectors are present but one has zero magnitude, so there is no angle between them.
    ZeroMagnitude,
    /// The two vectors have different lengths.
    LengthMismatch,
    /// **No similarity, and not a failure.** Either the two directions are orthogonal or one is opposed to
    /// the other; both score zero.
    ///
    /// This is a distinct variant rather than a zero score with no reason, because the score alone cannot
    /// distinguish it from a missing vector, an incomparable pair, or a zero magnitude. The distinction was
    /// found by a test: an orthogonal pair with a dot product of exactly `0.0` was being reported as an
    /// absence by a `cosine <= 0.0` guard, which conflates the two cases that produce that value.
    NoSimilarity,
}

impl SemanticAbsence {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NoIndex => "no_index",
            Self::NoQueryVector => "no_query_vector",
            Self::NoMemoryVector => "no_memory_vector",
            Self::Incomparable(_) => "incomparable",
            Self::ZeroMagnitude => "zero_magnitude",
            Self::LengthMismatch => "length_mismatch",
            Self::NoSimilarity => "no_similarity",
        }
    }
}

/// A semantic comparison's outcome: a score, or the reason there is none.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticOutcome {
    score: SignalScore,
    absence: Option<SemanticAbsence>,
}

impl SemanticOutcome {
    /// Returns the score, which is zero when there is no comparison.
    #[must_use]
    pub const fn score(&self) -> SignalScore {
        self.score
    }

    /// Returns why there is no comparison, or `None` when there was one.
    #[must_use]
    pub const fn absence(&self) -> Option<SemanticAbsence> {
        self.absence
    }
}

impl<'a> ScoringContext<'a> {
    /// Builds a context with no embedding index, so the semantic signal is zero everywhere.
    ///
    /// Named rather than defaulted, so a caller that leaves semantics out has written a word meaning it.
    #[must_use]
    pub const fn without_semantics() -> Self {
        Self { semantic: None }
    }

    /// Builds a context from a query vector and a lookup for a memory's vector.
    #[must_use]
    pub fn with_semantics(
        query: SemanticVector<'a>,
        vectors: impl Fn(&MemoryId) -> Option<SemanticVector<'a>> + 'a,
    ) -> Self {
        Self {
            semantic: Some(SemanticScoring {
                query,
                vectors: Box::new(vectors),
            }),
        }
    }

    /// Returns whether this context can produce a semantic score at all.
    #[must_use]
    pub const fn has_semantics(&self) -> bool {
        self.semantic.is_some()
    }

    /// Compares the query's vector against one memory's, or reports why it cannot.
    ///
    /// # Why the identity is compared before the arithmetic
    ///
    /// A cosine over two vectors from **different** models is a number with no meaning that still lands in
    /// the usual range, so a missing check does not look like a bug — it looks like a slightly worse
    /// ranking. `jarvis-models`' `ensure_comparable_with` performs the comparison and names the first field
    /// that differed, and this call site is where the domain hands it both sides.
    #[must_use]
    pub fn semantic_outcome(&self, memory_id: &MemoryId) -> SemanticOutcome {
        let Some(scoring) = &self.semantic else {
            return absent(SemanticAbsence::NoIndex);
        };
        let Some(memory) = (scoring.vectors)(memory_id) else {
            return absent(SemanticAbsence::NoMemoryVector);
        };
        let query = scoring.query;
        // Identity first: everything after this point is arithmetic that would "succeed" on two vectors
        // that have no business being compared.
        if query.provider != memory.provider {
            return absent(SemanticAbsence::Incomparable("provider"));
        }
        if query.model != memory.model {
            return absent(SemanticAbsence::Incomparable("model"));
        }
        if query.version != memory.version {
            return absent(SemanticAbsence::Incomparable("version"));
        }
        // A length mismatch is caught before the arithmetic because the dot product would truncate to the
        // shorter one without complaint, which is the failure a metadata guard exists to prevent.
        if query.values.len() != memory.values.len() {
            return absent(SemanticAbsence::LengthMismatch);
        }
        let denominator = if query.normalized == SemanticNormalization::Normalized
            && memory.normalized == SemanticNormalization::Normalized
        {
            1.0
        } else {
            let query_magnitude = magnitude(query.values);
            let memory_magnitude = magnitude(memory.values);
            if query_magnitude == 0.0 || memory_magnitude == 0.0 {
                return absent(SemanticAbsence::ZeroMagnitude);
            }
            query_magnitude * memory_magnitude
        };
        let dot = dot_product(query.values, memory.values);
        let cosine = (dot / denominator).clamp(-1.0, 1.0);
        // A zero or negative cosine scores zero rather than a negative signal: a component score is a
        // magnitude out of `SIGNAL_SCALE`, and treating an opposed vector as "less than no match" would let it
        // pull a total down, which no other signal can do and which the weights are not shaped for. The
        // reason is recorded rather than the score alone, so "no similarity" does not read as "not compared".
        if cosine <= 0.0 {
            return SemanticOutcome {
                score: 0,
                absence: Some(SemanticAbsence::NoSimilarity),
            };
        }
        // Rounded once, at the end, so the stored score is reproducible: `f32` arithmetic is not
        // associative, so scaling inside the loop would make the value depend on the component order. The
        // cosine is clamped to `[-1, 1]` above and the zero-or-negative case has returned, so the product is
        // provably in `[0, SIGNAL_SCALE]`.
        //
        // `as` between numeric types is normally denied because it truncates or wraps silently. It is allowed
        // here for one statement because the value's range is established by the two checks immediately above
        // it, and there is no fallible `f32`-to-integer conversion in the standard library to use instead.
        // A `try_from` on the *integer* would not help: the lossy step is the float conversion itself.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let score = (cosine * f32::from(SIGNAL_SCALE)).round() as SignalScore;
        SemanticOutcome {
            score,
            absence: None,
        }
    }
}

/// Returns a zero score with a reason.
const fn absent(reason: SemanticAbsence) -> SemanticOutcome {
    SemanticOutcome {
        score: 0,
        absence: Some(reason),
    }
}

/// Returns the Euclidean length of a vector.
fn magnitude(values: &[f32]) -> f32 {
    values.iter().map(|value| value * value).sum::<f32>().sqrt()
}

/// Returns the dot product of two vectors, over the shorter one.
fn dot_product(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f32>()
}

/// Computes every signal for one memory given a scoring context.
///
/// Each signal is a pure function of the record, the query, and the context, so a component can be
/// recomputed in isolation and the total cannot depend on the order signals were evaluated.
#[must_use]
pub fn signals_for(
    record: &MemoryRecord,
    query: &MemoryQuery,
    context: &ScoringContext<'_>,
) -> MemorySignals {
    MemorySignals {
        exact_identifier: exact_identifier_signal(record, query),
        keyword: keyword_signal(record, query),
        semantic: context.semantic_outcome(&record.id()).score(),
        entity_overlap: entity_overlap_signal(record, query),
        recency: recency_signal(record, query),
        temporal: temporal_signal(record, query),
        importance: importance_signal(record),
        source_reliability: source_reliability_signal(record),
        reinforcement: reinforcement_signal(record),
    }
}

/// Scores how completely the question's entities are the memory's entities.
///
/// The document's "exact identifiers and aliases" signal, and it is the strongest weight because it is the
/// only signal that answers the question the caller actually asked: a memory about the entity the question
/// names, rather than one whose text resembles the question. Full marks when **every** entity the question
/// named appears, so a memory about one of three named entities does not read as an exact hit â€” that case is
/// [`entity_overlap_signal`]'s.
#[must_use]
pub fn exact_identifier_signal(record: &MemoryRecord, query: &MemoryQuery) -> SignalScore {
    if query.entity_ids.is_empty() {
        return 0;
    }
    let matched = query
        .entity_ids
        .iter()
        .filter(|wanted| {
            record
                .entities()
                .iter()
                .any(|held| held.entity_id() == **wanted)
        })
        .count();
    if matched == query.entity_ids.len() {
        SIGNAL_SCALE
    } else {
        0
    }
}

/// Scores how much of the question's text appears in the memory.
///
/// # Why the words are compared as a set and the score is a proportion
///
/// The search key uses a sorted word set, and this signal uses the same relation so "the same claim" means
/// one thing across the module. The score is the fraction of the question's **distinct** words that appear,
/// so a long question is not penalised for the memory not repeating all of it, and a memory repeating a
/// question's common word does not score as a match. Counting distinct question words rather than all of
/// them is what stops a question's length from changing its scores.
///
/// Stop words are deliberately **not** removed, matching the search key's own rule: a list of stop words
/// would need maintenance per language, and getting it wrong silently changes which claims collide.
#[must_use]
pub fn keyword_signal(record: &MemoryRecord, query: &MemoryQuery) -> SignalScore {
    let wanted = distinct_words(&query.text);
    if wanted.is_empty() {
        return 0;
    }
    let held = distinct_words(record.content());
    let matched = wanted
        .iter()
        .filter(|word| held.iter().any(|candidate| candidate == *word))
        .count();
    scaled(matched, wanted.len())
}

/// Scores how much of the question's entity set the memory shares at all.
///
/// The weaker sibling of [`exact_identifier_signal`], for a memory that is about one of several entities
/// the question named. It is scored at the same time as the exact signal rather than instead of it, so a
/// memory about all named entities scores full on both â€” the weight difference is what makes the exact case
/// rank higher, and a branch that made them exclusive would put that decision in two places.
#[must_use]
pub fn entity_overlap_signal(record: &MemoryRecord, query: &MemoryQuery) -> SignalScore {
    if query.entity_ids.is_empty() {
        return 0;
    }
    let matched = query
        .entity_ids
        .iter()
        .filter(|wanted| {
            record
                .entities()
                .iter()
                .any(|held| held.entity_id() == **wanted)
        })
        .count();
    scaled(matched, query.entity_ids.len())
}

/// Scores how recent the memory is, relative to the query instant.
///
/// # Why this decays and why the window is a constant
///
/// The document lists both "recency and temporal relevance" and "reinforcement and prior useful
/// retrieval", and they answer different questions: recency is when the claim was recorded, reinforcement
/// is how often it has since been useful. This signal is the first, and it decays linearly over
/// [`RECENCY_WINDOW_NANOS`] so that a memory's score is comparable between two queries at different
/// instants â€” a step function would make every memory inside the window equal.
///
/// A memory created *after* the query instant scores full, which is not an error: a query instant in the
/// past is a legitimate "as of" question, and a claim recorded later was not available then. Scoring it
/// zero would silently drop it from a historical read where it is the *newest* fact available.
#[must_use]
pub fn recency_signal(record: &MemoryRecord, query: &MemoryQuery) -> SignalScore {
    let reference = record
        .last_accessed_at()
        .unwrap_or_else(|| record.created_at());
    let age = query.at.unix_nanos() - reference.unix_nanos();
    if age <= 0 {
        return SIGNAL_SCALE;
    }
    if age >= RECENCY_WINDOW_NANOS {
        return 0;
    }
    // Scaled before dividing, for the same reason as the keyword signal: the integer form of the fraction is
    // zero for every partial case otherwise. `age` is non-negative here by the guard above, and the window is
    // positive by its definition, so both conversions are exact.
    let remaining = u128::try_from(RECENCY_WINDOW_NANOS - age).unwrap_or_default();
    let window = u128::try_from(RECENCY_WINDOW_NANOS).unwrap_or(u128::MAX);
    let score = u128::from(SIGNAL_SCALE) * remaining / window;
    u16::try_from(score).unwrap_or(SIGNAL_SCALE)
}

/// How long it takes a memory's recency score to reach zero: thirty days.
///
/// A constant rather than a per-query parameter, because a window that could vary per call would make two
/// scores incomparable and a stored score unreproducible â€” which is the same argument the scoring doc gives
/// for integers over floats. A caller with a different horizon expresses it as a validity window on the
/// claim, which is where a time-bounded fact belongs.
pub const RECENCY_WINDOW_NANOS: i128 = 30 * 24 * 60 * 60 * 1_000_000_000;

/// Scores whether the claim is inside its validity window at the query instant.
///
/// # Why this is separate from eligibility's currency check
///
/// Eligibility refuses a claim that is **not current** â€” superseded, expired, or deleted. This signal is
/// about a claim that *is* current and whose window the instant happens to fall inside, which is a ranking
/// difference rather than a filter: a preference with no end date and a fact that expires next week are
/// both current, and the second is more relevant to a question asked today. Both are needed, and merging
/// them would either drop current claims or rank expired ones.
#[must_use]
pub fn temporal_signal(record: &MemoryRecord, query: &MemoryQuery) -> SignalScore {
    let from = record.valid_from().unix_nanos();
    let until = record.valid_until().map(UtcTimestamp::unix_nanos);
    let at = query.at.unix_nanos();
    if at < from {
        // Not yet true at the query instant: a claim about a future state, which answers a question about
        // the future and not one about now.
        return 0;
    }
    match until {
        // An end date the instant has reached, or an open-ended claim: the first scores zero because the
        // window closed, the second full because nothing about the instant makes a preference less
        // applicable.
        Some(until) if at >= until => 0,
        Some(_) | None => SIGNAL_SCALE,
    }
}

/// Scales the memory's declared importance rank to the signal range.
///
/// The rank is bounded at [`crate::memory::MAX_MEMORY_IMPORTANCE`] by the domain, so the divisor is that
/// bound rather than the highest rank actually present â€” a divisor derived from the candidate set would
/// make one memory's score depend on the others, which is exactly the coupling the component scores exist
/// to avoid.
#[must_use]
pub fn importance_signal(record: &MemoryRecord) -> SignalScore {
    scaled(
        usize::from(record.importance()),
        usize::from(crate::memory::MAX_MEMORY_IMPORTANCE),
    )
}

/// Scales what the memory's origin can be asked about to the signal range.
///
/// The document's "source reliability". It is derived from the **kind**, not from the stored trust, because
/// the kind is the fact and the trust is a property the kind grants â€” `P4-001` refuses a kind whose trust
/// disagrees, so deriving it here cannot contradict a stored value. The three trust levels are the scale.
///
/// The confidence is deliberately **not** part of this signal: confidence is already eligibility's minimum
/// and a separate signal would count one fact twice, which inflates a well-supported claim's rank relative
/// to one whose origin is merely good.
#[must_use]
pub fn source_reliability_signal(record: &MemoryRecord) -> SignalScore {
    match record.source().trust() {
        MemoryTrust::Authoritative => SIGNAL_SCALE,
        MemoryTrust::Derived => SIGNAL_SCALE / 2,
        MemoryTrust::Untrusted => 0,
    }
}

/// Scores how often the memory has been usefully retrieved.
///
/// The document's "reinforcement and prior useful retrieval". Saturating at
/// [`REINFORCEMENT_SATURATION`] rather than growing without bound: a memory retrieved a hundred times is
/// not ten times as useful as one retrieved ten times, and an unbounded term would let a single old, often
/// used memory outrank everything current â€” the "dominate by accident" failure, reached by accumulation
/// rather than by one weight.
#[must_use]
pub fn reinforcement_signal(record: &MemoryRecord) -> SignalScore {
    scaled(
        usize::try_from(record.retrieval_count()).unwrap_or(usize::MAX),
        usize::try_from(REINFORCEMENT_SATURATION).unwrap_or(1),
    )
}

/// How many useful retrievals a memory needs to reach full reinforcement.
pub const REINFORCEMENT_SATURATION: u32 = 8;

/// Derives the inclusion reason from the component scores.
///
/// # Why the reason is derived rather than passed in
///
/// The document requires the reason be stored, and computing it beside the scores would let the two
/// disagree â€” a memory reported as a keyword match while its keyword signal is zero. Deriving it from the
/// components makes that unrepresentable and makes the reason *verifiable*: a test asserts it against the
/// contribution table rather than against a second copy of the same rule.
///
/// The precedence is **strongest matching signal first**, then the two non-matching reasons. The order
/// between equal contributions is fixed by the signal order, so a tie is not a coin flip, and a memory that
/// matched nothing is reported as [`SelectionReason::RecentAndCurrent`] or
/// [`SelectionReason::Important`] rather than as a match â€” which is what
/// [`SelectionReason::is_a_match`] exists to expose.
#[must_use]
pub fn reason_for(signals: &MemorySignals, weights: &SignalWeights) -> SelectionReason {
    // Specificity order, most specific first: the identifier, then the partial overlap, then the words, then
    // meaning.
    //
    // The comparison is written out rather than `max_by_key` because the two are usually EQUAL — a memory
    // about an entity a question names usually also shares the question's words, scoring full on both —
    // and `max_by_key` returns the **last** maximum on a tie. So the reason would depend on the order of an
    // array literal rather than on a decision, and a memory scoring full on all four would be reported as
    // whatever came last. `>` rather than `>=` makes the first signal at a given score win, so this order is
    // the precedence.
    //
    // Semantic similarity is **last** among the matching signals even though the document lists it above
    // entity overlap. The reason is the one a user is told, and "this memory means something close to what
    // you asked" is a weaker claim than "this memory is about the entity you named" or even "this memory
    // repeats your words" — the first is an inference, and the other two are present in the text. The
    // *weight* keeps the document's order, because a weight says how much a signal contributes rather than
    // how confidently its result can be described.
    let mut best: Option<(SelectionReason, SignalScore)> = None;
    for (reason, score) in [
        (SelectionReason::ExactIdentifier, signals.exact_identifier),
        (SelectionReason::EntityOverlap, signals.entity_overlap),
        (SelectionReason::KeywordMatch, signals.keyword),
        (SelectionReason::SemanticSimilarity, signals.semantic),
    ] {
        if score > 0 && best.is_none_or(|(_, held)| score > held) {
            best = Some((reason, score));
        }
    }
    if let Some((reason, _)) = best {
        return reason;
    }
    // Nothing matched: the memory is offered for its currency and rank, and which of the two is named is
    // decided by which contributed more, so the reason points at the stronger justification. The two are
    // compared as weighted contributions rather than as raw scores, because the weights decide which
    // justification is stronger.
    let importance =
        u32::from(signals.importance) * u32::from(weights.importance) / u32::from(SIGNAL_SCALE);
    let currency = (u32::from(signals.recency) * u32::from(weights.recency)
        + u32::from(signals.temporal) * u32::from(weights.temporal))
        / u32::from(SIGNAL_SCALE);
    if importance > currency {
        SelectionReason::Important
    } else {
        SelectionReason::RecentAndCurrent
    }
}

/// Splits content into distinct words, whitespace-collapsed and ASCII-case-folded.
///
/// The same normalization the search key uses, deliberately: if the two disagreed, a claim could match a
/// query's words and not its own key, and "which memories are about this" would have two answers.
fn distinct_words(content: &str) -> Vec<String> {
    let mut words: Vec<String> = content
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    words.sort_unstable();
    words.dedup();
    words
}

/// Returns whether a memory's source kind may be offered as an answer at all.
///
/// A convenience for a caller that wants the document's "supported by source?" question without building a
/// query, and stated here so the two uses cannot disagree: a model inference is never eligible as a fact,
/// which is the same rule `P4-003`'s pipeline applies at admission.
#[must_use]
pub const fn may_be_offered_as_fact(kind: MemorySourceKind) -> bool {
    !kind.is_model_produced()
}

#[cfg(test)]
#[path = "retrieval/tests.rs"]
mod tests;
