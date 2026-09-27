//! Candidate extraction: what a model may propose, and what deterministic code decides.
//!
//! `docs/architecture/memory-and-context.md` defines the admission lifecycle as a pipeline, and its
//! second paragraph states the division of labour this module implements literally:
//!
//! > The model may propose candidates, labels, confidence, and entities. Deterministic code validates
//! > shape, source linkage, workspace, size, sensitivity, and retention.
//!
//! So a [`MemoryCandidate`] is **not** a `MemoryRecord` with fewer fields. It is the model's
//! *proposal*, and it is deliberately not a domain value: it can carry a confidence the source cannot
//! support, a sensitivity below the floor the content implies, and an entity set the store has not
//! resolved. Each of those is something a stage resolves or refuses.
//!
//! # The stages, and why they are named
//!
//! [`MemoryCandidate::admit`] runs the document's lifecycle in its own order. The stages are separate
//! because the pipeline's interesting output is the **reason**: "the model's confidence exceeded what
//! its source can support" and "the workspace already holds this claim" are the same boolean, and an
//! operator asking why something was not remembered needs to tell them apart.
//!
//! The order is load-bearing in three places:
//!
//! 1. **Shape before support**, because support reads the classification and a length rule needs
//!    nothing else.
//! 2. **Entities before the key**, because the key is derived from the resolved entities. Deriving it
//!    from the extractor's proposals would compare a different key than the one the store holds.
//! 3. **The tombstone before the duplicate**, because a deleted memory's row has no search key and so
//!    cannot be found as an existing claim. `P4-002`'s `record_memory` needs the same ordering, for
//!    the same reason, and both are stated so a reader of either can see why.
//!
//! # What this module deliberately does not do
//!
//! **It does not write.** `admit` takes `&self` and returns an admission; persisting is the caller's
//! act. A pipeline that stored as it went could not be inspected before it wrote, and the document's
//! "do not persist as fact" step is only meaningful if persistence is separate.
//!
//! **It does not call a model, and it does not read the store.** Extraction from a conversation is
//! `P4-007`'s. What the store knows arrives as borrowed values — a resolved entity set, the current
//! memory for a key, a replacement, and a tombstone answer — because resolution needs the entity
//! repository and this crate has no storage dependency.

use crate::id::{MemoryId, RunId, WorkspaceId};
use crate::memory::{
    EntityRef, MAX_MEMORY_CONTENT_CHARS, MAX_MEMORY_ENTITIES, MemoryConfidence, MemoryRecord,
    MemorySearchKey, MemorySourceKind, MemoryTrust, MemoryType, StructuredClaim,
};
use crate::sensitivity::Sensitivity;

/// The longest a candidate's proposed content may be.
///
/// Equal to [`MAX_MEMORY_CONTENT_CHARS`] rather than larger: content longer than a memory can hold can
/// never become one, so accepting it would defer the refusal to a later stage with less to say about
/// why. A shorter bound would refuse content the store could hold.
pub const MAX_CANDIDATE_CONTENT_CHARS: usize = MAX_MEMORY_CONTENT_CHARS;

/// The most entities a candidate may propose.
pub const MAX_CANDIDATE_ENTITIES: usize = MAX_MEMORY_ENTITIES;

/// Why a candidate was not admitted as a plain new memory.
///
/// One variant per cause, carrying the values the decision used. The values are the point: an operator
/// asking "why was this not remembered" needs "the model claimed more confidence than its source can
/// support" to be actionable, and a boolean is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRefusal {
    /// The proposed content was empty or longer than a memory may be.
    Content,
    /// The content yields no usable search key.
    ///
    /// A **different rule from [`Self::Content`]**, and separated because the two are fixed differently: the
    /// content bound is about length, while this is about the key's own shape — a claim whose text is one
    /// unbroken word longer than [`crate::memory::MAX_SEARCH_KEY_WORD_CHARS`], or one with more than
    /// [`crate::memory::MAX_SEARCH_KEY_WORDS`] distinct words, cannot be keyed. Reporting "content" for both
    /// would send an operator shortening text that is already within the bound.
    Unkeyable,
    /// The source locator was empty.
    ///
    /// A claim with no source is not a memory: the canonical record requires `source_id`, and the
    /// document's admission step rejects what is "not supported by source" rather than storing it
    /// unsourced.
    Unsupported,
    /// More than [`MAX_CANDIDATE_ENTITIES`] entities were proposed or resolved.
    TooManyEntities,
    /// Entity resolution produced no entity.
    ///
    /// A refusal rather than a fallback to the extractor's proposals: an unresolved entity is precisely
    /// what the document says must "remain separate candidates", and a claim whose subject is a guess
    /// would be a claim about whoever the guess turned out to be.
    EntityUnresolved,
    /// The confidence was **lowered** to what the source can support.
    ///
    /// Present in the vocabulary because a caller told only "admitted" would not learn that the stored
    /// confidence differs from the one proposed. This is the "an inferred preference never appears as
    /// confirmed fact" property becoming *visible* instead of silently corrected.
    ConfidenceLowered {
        /// The confidence the candidate proposed.
        proposed: MemoryConfidence,
        /// The highest confidence the source class may carry.
        ceiling: MemoryConfidence,
    },
    /// A provider record cannot back a preference, whatever the provider is authoritative about.
    ProviderCannotPreference,
    /// The workspace holds this exact claim already.
    ///
    /// **Not a failure.** The caller reinforces the existing memory instead of writing a second row.
    Duplicate {
        /// The memory that already holds the claim.
        existing_memory_id: MemoryId,
    },
    /// The workspace held this claim and a later memory replaced it.
    ///
    /// Distinct from a duplicate because the current claim is *not* the candidate's claim: reinforcing
    /// it would count a retrieval the candidate did not cause.
    AlreadySuperseded {
        /// The memory that holds the claim now.
        current_memory_id: MemoryId,
    },
    /// The workspace holds a different claim at this key, which the candidate replaces.
    ///
    /// **Not a failure either.** The document requires it be recorded as its own claim with a
    /// `supersedes` link: "never overwrite contradictory memory silently".
    Correction {
        /// The memory the candidate corrects.
        supersedes: MemoryId,
    },
    /// The claim was deleted in this workspace and must not return.
    Tombstoned,
}

impl CandidateRefusal {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Unkeyable => "unkeyable",
            Self::Unsupported => "unsupported",
            Self::TooManyEntities => "too_many_entities",
            Self::EntityUnresolved => "entity_unresolved",
            Self::ConfidenceLowered { .. } => "confidence_lowered",
            Self::ProviderCannotPreference => "provider_cannot_preference",
            Self::Duplicate { .. } => "duplicate",
            Self::AlreadySuperseded { .. } => "already_superseded",
            Self::Correction { .. } => "correction",
            Self::Tombstoned => "tombstoned",
        }
    }

    /// Returns whether the refusal means nothing may be written at all.
    ///
    /// The distinction a caller needs, because four of these are **outcomes with an action** rather
    /// than failures: a duplicate is reinforced, a superseded claim is left alone, a correction
    /// supersedes, and a lowered confidence is written as the ceiling allows. `true` here means this
    /// refusal is terminal and the candidate produced no work.
    #[must_use]
    pub const fn forbids_writing(&self) -> bool {
        !matches!(
            self,
            Self::ConfidenceLowered { .. }
                | Self::Duplicate { .. }
                | Self::AlreadySuperseded { .. }
                | Self::Correction { .. }
        )
    }
}

/// The highest confidence a source class can support.
///
/// # Why this is a function of the kind rather than of a trust level
///
/// Trust and confidence answer different questions and the document says so: trust is whether the
/// origin is authoritative for this *class* of claim, while confidence is how well *this* claim is
/// supported. So the ceiling tracks what the source can be asked about.
///
/// - **`Confirmed`** — the record is the evidence. The user's own words, a provider's own row, a tool's
///   own output. Nothing about the claim can be better established than the record of it.
/// - **`Likely`** — the text is the record but its *meaning* was read. A document says what it says;
///   what it implies is an interpretation.
/// - **`Uncertain`** — the claim reconstructs content from outside the trust boundary. Even a faithful
///   quotation supports only what that content said, not that it is true.
/// - **`Unverified`** — nothing supports the claim beyond having been produced. This is the model
///   inference row, and "an inferred preference never appears as confirmed fact" is exactly it.
#[must_use]
pub const fn confidence_ceiling(kind: MemorySourceKind) -> MemoryConfidence {
    match kind {
        MemorySourceKind::UserStatement
        | MemorySourceKind::UserCorrection
        | MemorySourceKind::ProviderRecord
        | MemorySourceKind::ToolObservation => MemoryConfidence::Confirmed,
        MemorySourceKind::Document => MemoryConfidence::Likely,
        MemorySourceKind::ExternalContent => MemoryConfidence::Uncertain,
        MemorySourceKind::ModelInference => MemoryConfidence::Unverified,
    }
}

/// What a candidate is: the type the extractor proposed, and the source it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateClassification {
    /// What kind of memory the candidate would be.
    pub memory_type: MemoryType,
    /// Where the claim came from.
    pub source_kind: MemorySourceKind,
}

impl CandidateClassification {
    /// Returns the highest confidence this classification can support.
    #[must_use]
    pub const fn confidence_ceiling(&self) -> MemoryConfidence {
        confidence_ceiling(self.source_kind)
    }

    /// Returns whether the claim must be stored as a proposal rather than as current truth.
    ///
    /// Two independent reasons and either is sufficient, which is why this is one predicate: the type
    /// requires confirmation (a relationship claim), or the source is a model inference — and a model
    /// inference is a proposal **at any confidence**, so lowering the confidence cannot make it a fact.
    #[must_use]
    pub const fn requires_proposal(&self) -> bool {
        self.memory_type.requires_confirmation() || self.source_kind.is_model_produced()
    }
}

/// The sensitivity floor a memory type implies, before the content is read.
///
/// # Why a floor exists rather than taking what the extractor said
///
/// `Sensitivity::Internal` is both the default and this floor, because a classification that was never
/// set must not be treated as safe to disclose — the type's own documented default. One case sits
/// above it: a **relationship claim is `Confidential`**, because the document's table calls this type
/// "long-lived, sensitive" and the claim is about a person rather than about a preference.
///
/// Everything else is `Internal`, deliberately. Raising the floor for ordinary claims would make the
/// level meaningless, and a floor's cost is real: a floor is content withheld from a remote model.
///
/// # Why this is separate from the content's floor
///
/// The two answer different questions about different things — this one is about the claim's *class*,
/// [`content_sensitivity_floor`] about its *text* — and keeping them apart says so. A single function
/// blending them would make "a relationship claim is confidential even when the text is innocuous"
/// unstateable.
#[must_use]
pub const fn type_sensitivity_floor(memory_type: MemoryType) -> Sensitivity {
    if matches!(memory_type, MemoryType::Relationship) {
        return Sensitivity::Confidential;
    }
    Sensitivity::Internal
}

/// Detects credential- or health-shaped content, raising the floor to `Restricted`.
///
/// # What this is, and what it is not
///
/// It is a **pattern check on the text**, and it exists because a claim can disclose a secret by
/// carrying it: "my API key is sk-…" is a `Semantic` claim whose sensitivity the *type* says nothing
/// about, and the same is true of a diagnosis. The document names credentials and health data as its
/// examples of `Restricted`.
///
/// It is deliberately conservative and it is **not** a classifier. Nothing depends on it being
/// complete, because it can only raise the level: a miss leaves the caller's own classification in
/// place, while a hit is a disclosure prevented. A secret that looks like ordinary prose is caught by
/// nothing here, and that limit is recorded rather than papered over.
///
/// Returns `Public` for content it does not recognize, so the result is usable as a floor that changes
/// nothing — returning `Internal` would silently raise every candidate to the default.
#[must_use]
pub fn content_sensitivity_floor(content: &str) -> Sensitivity {
    // Prefixes of credential shapes. A claim carrying a live key usually carries its prefix.
    const SECRET_PREFIXES: [&str; 8] = [
        "sk-",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "akia",
        "-----begin ",
    ];
    // Words naming a class of data the document calls restricted. A claim that *names* health or
    // credential data is treated as carrying it: "my diagnosis is X" and "I have a diagnosis" are not a
    // distinction a keyword check can draw, and the two mistakes are not symmetric.
    const RESTRICTED_WORDS: [&str; 10] = [
        "password",
        "passphrase",
        "api key",
        "apikey",
        "secret",
        "token",
        "credential",
        "diagnosis",
        "medication",
        "prescription",
    ];

    let lowered = content.to_ascii_lowercase();
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| lowered.contains(prefix))
        || RESTRICTED_WORDS.iter().any(|word| lowered.contains(word))
    {
        Sensitivity::Restricted
    } else {
        Sensitivity::Public
    }
}

/// The sensitivity a candidate's memory must carry: the highest of the three floors.
///
/// # Why the maximum, and why the extractor's value participates
///
/// Three sources each know something the others do not: the **type** knows a relationship claim is
/// sensitive, the **content** knows a credential is present, and the **extractor** may know something
/// about the context that neither can see. `Sensitivity` is ordered for exactly this comparison, and the
/// maximum is the only safe direction — over-classifying withholds content from a remote model, a
/// usability cost, while under-classifying is a disclosure.
///
/// So the proposed value is used when it is **at least** every floor and ignored when it is below one.
/// The alternative — trust the extractor and correct it at the call site — puts the disclosure decision
/// where nothing records it, and the document's rule is destination-facing: "sensitive memories can be
/// excluded from remote models" is decided from the *stored* level, so a level below a floor is a
/// disclosure no later check recovers.
#[must_use]
pub fn classify_sensitivity(
    memory_type: MemoryType,
    proposed: Sensitivity,
    content: &str,
) -> Sensitivity {
    // Computed as successive maxima rather than `std::cmp::max` so the direction is visible in the code:
    // a value is raised to a floor and never lowered to one.
    let mut effective = type_sensitivity_floor(memory_type);
    let content_floor = content_sensitivity_floor(content);
    if content_floor.level() > effective.level() {
        effective = content_floor;
    }
    if proposed.level() > effective.level() {
        effective = proposed;
    }
    effective
}

/// One proposed memory, before the pipeline's rules have been applied.
///
/// # Why this is a separate type from `MemoryRecord`
///
/// A `MemoryRecord` cannot be built until every rule passes — `P4-001` made that structural, so there is
/// no way to hold a half-valid one. A candidate *is* the value before those rules, so it has to be
/// representable without them, and keeping it distinct is what makes "the model proposed this" sayable.
/// Merging them, for instance by making the record's content optional, would make every consumer of a
/// memory defend against a half-valid value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryCandidate {
    /// The workspace the claim would belong to, which is also the retrieval boundary.
    pub workspace_id: WorkspaceId,
    /// The proposed useful text.
    pub content: String,
    /// What the candidate is: its type and its source.
    pub classification: CandidateClassification,
    /// The sensitivity the extractor proposed, used only when it is at least every floor.
    pub proposed_sensitivity: Sensitivity,
    /// The confidence the extractor proposed, capped by the source's ceiling.
    pub proposed_confidence: MemoryConfidence,
    /// How much it matters, as a bounded rank.
    pub importance: u8,
    /// The entities the extractor proposed, unresolved.
    pub proposed_entities: Vec<EntityRef>,
    /// The claim in normalized form, when the extractor could produce one.
    pub structured_claim: Option<StructuredClaim>,
    /// The source's locator: a session, run, document, or provider reference.
    pub source_locator: String,
    /// The source's digest, when the extractor has one.
    pub source_excerpt_hash: Option<String>,
    /// The run that produced the candidate, when one did.
    pub run_id: Option<RunId>,
    /// The memory this candidate **declares** that it corrects, when it corrects one.
    ///
    /// # Why a correction is declared and not inferred
    ///
    /// The obvious design — "the workspace holds a different claim about the same entity, so this one
    /// corrects it" — cannot be implemented, and the reason is `P4-002`'s search key: the key **is** the
    /// sorted, case-folded word set, so two claims sharing a key have the same words and cannot be
    /// different claims. So a key match is always the *same* claim, and a different claim always has a
    /// different key and therefore no `existing` memory to compare against.
    ///
    /// Inferring a correction from "same entity, different words" was the next candidate and it is worse:
    /// two unrelated facts about one person ("Alice is my sister", "Alice lives in Rotterdam") share an
    /// entity and differ in words, so every second fact about anyone would retire the first.
    ///
    /// So supersession is **stated**: by the user ("I prefer tea now"), or by whoever decided the new claim
    /// replaces the old one. Only that party knows the two are the same subject area, and a pipeline that
    /// guessed would either miss corrections or invent them.
    pub supersedes: Option<MemoryId>,
    /// The actor that produced it.
    pub created_by_actor_id: String,
    /// The correlation identity shared with the originating request.
    pub correlation_id: crate::id::CorrelationId,
}

/// What the caller knows about the store, so the pipeline can reason without reading it.
///
/// # Why this is a struct rather than four parameters
///
/// The facts are read from the same place in the same order — resolve the entities, look up the key,
/// look up any replacement, look up the tombstone — and three of the four are `Option`-shaped or
/// identifier-typed. Positional arguments would be a call site where a memory identifier and a run
/// identifier could be transposed without a type error, which is the hazard the repository's own
/// construction sites already group their parameters to avoid.
///
/// The lifetime is on the borrow of what the store answered, so the caller keeps ownership and the
/// pipeline cannot outlive its evidence.
#[derive(Clone, Copy, Debug)]
pub struct CandidateContext<'a> {
    /// The entities the store resolved, which replace whatever the extractor proposed.
    ///
    /// An empty slice means resolution produced nothing, which is a refusal rather than an invitation to
    /// fall back on the proposals — see [`CandidateRefusal::EntityUnresolved`].
    pub resolved_entities: &'a [EntityRef],
    /// The workspace's current memory for the candidate's key, when it holds one.
    pub existing: Option<&'a MemoryRecord>,
    /// The memory that replaced the claim at this key, when a replacement exists.
    pub superseded_by: Option<MemoryId>,
    /// Whether the claim was deleted in this workspace.
    pub tombstoned: bool,
}

/// What the pipeline decided should happen.
///
/// # Why the reason travels inside the value rather than beside it
///
/// Every admission has a reason, and four of them are "write something, but not a plain new memory".
/// Returning a `Result` with a separate `Option<CandidateRefusal>` would let a caller read the decision
/// and ignore the reason, and the reason is what decides the write. One value means the action and its
/// justification cannot be separated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryAdmission {
    /// Write the candidate as a current memory.
    New(MemoryToStore),
    /// Write the candidate as a proposal requiring confirmation.
    ///
    /// The document's "store / supersede / propose review", and the outcome for a model inference and for
    /// a relationship claim. `P4-001`'s constructor derives the `Proposed` status from the type, so
    /// writing one of these and reading it back is what proves the two layers agree rather than merely
    /// intending to.
    Proposal(MemoryToStore),
    /// Write the candidate, linking it to the memory it corrects.
    Correction {
        /// The values to store.
        to_store: MemoryToStore,
        /// The memory this one supersedes.
        supersedes: MemoryId,
    },
    /// Do not write; reinforce the memory that already holds the claim.
    Duplicate {
        /// The memory to reinforce.
        existing_memory_id: MemoryId,
    },
    /// Do not write; a newer memory already holds this claim.
    AlreadySuperseded {
        /// The memory that holds the claim now.
        current_memory_id: MemoryId,
    },
}

impl MemoryAdmission {
    /// Returns the values to store, when the admission calls for a write.
    #[must_use]
    pub const fn to_store(&self) -> Option<&MemoryToStore> {
        match self {
            Self::New(to_store) | Self::Proposal(to_store) | Self::Correction { to_store, .. } => {
                Some(to_store)
            }
            Self::Duplicate { .. } | Self::AlreadySuperseded { .. } => None,
        }
    }

    /// Returns the reason this admission is not a plain new memory, when it is not.
    ///
    /// A plain [`Self::New`] and a [`Self::Proposal`] have nothing to report beyond their variant — a
    /// proposal's reason is its type or its source, which the caller already holds — so this is an
    /// `Option` rather than a mandatory field: synthesizing a "reason: new" would put the field at every
    /// call site and make it read at none. The identifier is the reason for the three that carry one, and
    /// deriving it here is what keeps the value and its justification from being able to disagree.
    #[must_use]
    pub const fn reason(&self) -> Option<CandidateRefusal> {
        match self {
            Self::New(_) | Self::Proposal(_) => None,
            Self::Correction { supersedes, .. } => Some(CandidateRefusal::Correction {
                supersedes: *supersedes,
            }),
            Self::Duplicate {
                existing_memory_id, ..
            } => Some(CandidateRefusal::Duplicate {
                existing_memory_id: *existing_memory_id,
            }),
            Self::AlreadySuperseded {
                current_memory_id, ..
            } => Some(CandidateRefusal::AlreadySuperseded {
                current_memory_id: *current_memory_id,
            }),
        }
    }

    /// Returns whether the admission calls for a write.
    #[must_use]
    pub const fn writes(&self) -> bool {
        self.to_store().is_some()
    }

    /// Returns whether the write must land as a proposal requiring confirmation.
    #[must_use]
    pub const fn is_proposal(&self) -> bool {
        matches!(self, Self::Proposal(_))
    }
}

/// The values a caller needs to build a `MemoryRecord`, with every pipeline decision already applied.
///
/// # Why the applied values are returned rather than the candidate
///
/// Returning the candidate would leave the caller to re-derive the confidence cap, the sensitivity
/// floor, and whether the claim is a proposal — three decisions this module exists to make once.
/// Returning them applied means the caller's `MemoryRecord::new` cannot disagree with the pipeline, and
/// it is what makes the pipeline's output testable without a store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryToStore {
    /// The workspace the memory belongs to.
    pub workspace_id: WorkspaceId,
    /// What kind of memory this is.
    pub memory_type: MemoryType,
    /// The content to store, trimmed.
    pub content: String,
    /// The claim in normalized form, when one was derivable.
    pub structured_claim: Option<StructuredClaim>,
    /// The source kind.
    pub source_kind: MemorySourceKind,
    /// The source's locator, trimmed.
    pub source_locator: String,
    /// The source's trust class, derived from its kind rather than proposed.
    pub source_trust: MemoryTrust,
    /// The source's digest, when one exists.
    pub source_excerpt_hash: Option<String>,
    /// The confidence **after** the ceiling was applied.
    pub confidence: MemoryConfidence,
    /// The sensitivity **after** every floor was applied.
    pub sensitivity: Sensitivity,
    /// How much it matters, bounded.
    pub importance: u8,
    /// The entities resolution produced.
    pub entities: Vec<EntityRef>,
    /// The search key, derived from the stored type, the resolved entities, and the trimmed content.
    pub search_key: String,
    /// The run that produced it.
    pub run_id: Option<RunId>,
    /// The actor that produced it.
    pub created_by_actor_id: String,
    /// The correlation identity.
    pub correlation_id: crate::id::CorrelationId,
}

impl MemoryCandidate {
    /// Applies the admission lifecycle and returns what should happen.
    ///
    /// # Errors
    ///
    /// Returns the [`CandidateRefusal`] that stopped the candidate. A refusal whose
    /// [`CandidateRefusal::forbids_writing`] is `false` describes an outcome with an action rather than a
    /// failure, so a caller should branch on the variant rather than treat every `Err` as terminal. The
    /// three comparison outcomes never appear here: they are [`MemoryAdmission`] variants, because a
    /// duplicate and a correction each call for a write of their own kind rather than for nothing.
    pub fn admit(
        &self,
        context: &CandidateContext<'_>,
    ) -> Result<MemoryAdmission, CandidateRefusal> {
        // Stage 1: shape. Bounds and a source, before anything consults the store.
        let trimmed = self.content.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_CANDIDATE_CONTENT_CHARS {
            return Err(CandidateRefusal::Content);
        }
        if self.source_locator.trim().is_empty() {
            return Err(CandidateRefusal::Unsupported);
        }
        if self.proposed_entities.len() > MAX_CANDIDATE_ENTITIES
            || context.resolved_entities.len() > MAX_CANDIDATE_ENTITIES
        {
            return Err(CandidateRefusal::TooManyEntities);
        }

        // Stage 2: the entities the store resolved, before the key is derived from them. Proposals are
        // deliberately not a fallback: an unresolved entity is a guess, and the key built from it would
        // name an entity the store does not have.
        if context.resolved_entities.is_empty() {
            return Err(CandidateRefusal::EntityUnresolved);
        }

        // Stage 3: what the source can support. The provider/preference rule comes first because it is a
        // rule about the pair rather than about the confidence, so reporting a lowered confidence for a
        // candidate that can never be stored would name the wrong cause.
        if matches!(
            self.classification.source_kind,
            MemorySourceKind::ProviderRecord
        ) && matches!(self.classification.memory_type, MemoryType::Preference)
        {
            return Err(CandidateRefusal::ProviderCannotPreference);
        }
        let ceiling = self.classification.confidence_ceiling();
        // Lowered rather than refused, and only ever downward. A higher proposed confidence is the
        // extractor's optimism about the *same* claim, so discarding it would lose a claim the user would
        // want, while admitting it would let an inference read as established.
        let confidence = if self.proposed_confidence.level() > ceiling.level() {
            ceiling
        } else {
            self.proposed_confidence
        };

        // Stage 4: the key, derived from the resolved entities and the trimmed content. A failure here is a
        // rule about the key's shape rather than about the content's length, so it has its own refusal.
        let search_key = MemorySearchKey::new(
            self.classification.memory_type,
            context.resolved_entities,
            trimmed,
        )
        .map_err(|_| CandidateRefusal::Unkeyable)?;

        // Stage 5: the tombstone, **before** the duplicate check. A deleted memory's row has no search
        // key, so it cannot appear as `context.existing` and the ordering is what makes the deletion
        // reachable at all. `P4-002`'s `record_memory` documents the same ordering for the same reason.
        if context.tombstoned {
            return Err(CandidateRefusal::Tombstoned);
        }

        let to_store = MemoryToStore {
            workspace_id: self.workspace_id,
            memory_type: self.classification.memory_type,
            content: trimmed.to_owned(),
            structured_claim: self.structured_claim.clone(),
            source_kind: self.classification.source_kind,
            source_locator: self.source_locator.trim().to_owned(),
            source_trust: self.classification.source_kind.permitted_trust(),
            source_excerpt_hash: self.source_excerpt_hash.clone(),
            confidence,
            sensitivity: classify_sensitivity(
                self.classification.memory_type,
                self.proposed_sensitivity,
                trimmed,
            ),
            importance: self.importance.min(crate::memory::MAX_MEMORY_IMPORTANCE),
            entities: context.resolved_entities.to_vec(),
            search_key: search_key.as_str().to_owned(),
            run_id: self.run_id,
            created_by_actor_id: self.created_by_actor_id.clone(),
            correlation_id: self.correlation_id,
        };

        // Stage 6: the declared correction. Checked before the comparison because it is an *assertion about
        // the candidate's own history* rather than an observation about the store, and it outranks a
        // proposal: a corrected relationship claim must supersede, not become a proposal, or the obsolete
        // claim stays current truth.
        //
        // A self-supersession needs no check here, unlike in `MemoryRecord::new`: the candidate carries no
        // identifier, because a memory's identifier is assigned when it is stored. So the value that could
        // name itself does not exist at this stage, and the domain's own refusal covers the case once one does.
        if let Some(supersedes) = self.supersedes {
            return Ok(MemoryAdmission::Correction {
                to_store,
                supersedes,
            });
        }

        // Stage 7: the comparison against what the workspace holds at this key.
        match compare(context.existing, trimmed, context.superseded_by) {
            MemoryCandidateComparison::Correction { supersedes } => {
                return Ok(MemoryAdmission::Correction {
                    to_store,
                    supersedes,
                });
            }
            MemoryCandidateComparison::Duplicate { existing_memory_id } => {
                return Ok(MemoryAdmission::Duplicate { existing_memory_id });
            }
            MemoryCandidateComparison::AlreadySuperseded { current_memory_id } => {
                return Ok(MemoryAdmission::AlreadySuperseded { current_memory_id });
            }
            MemoryCandidateComparison::New => {}
        }

        // Stage 8: fact or proposal. A model inference and a relationship claim are never written as
        // current truth, and the reason is derived here rather than left to the caller.
        if self.classification.requires_proposal() {
            return Ok(MemoryAdmission::Proposal(to_store));
        }
        Ok(MemoryAdmission::New(to_store))
    }
}

/// The comparison of a candidate against what the workspace already holds.
///
/// # Why four outcomes rather than "is it a duplicate"
///
/// The document requires both halves of this and they pull in opposite directions: "deduplicate /
/// compare existing" and "never overwrite contradictory memory silently … create a new claim, link
/// `supersedes`". A boolean cannot express them, and the two mistakes it forces are both real — a
/// duplicate treated as a correction retires a claim for nothing, while a correction treated as a
/// duplicate leaves the obsolete claim as current truth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryCandidateComparison {
    /// The workspace holds nothing at this key.
    New,
    /// The workspace holds this exact claim, current. Reinforce it rather than writing a second row.
    Duplicate {
        /// The memory that already holds the claim.
        existing_memory_id: MemoryId,
    },
    /// The workspace held this claim and a later memory replaced it.
    AlreadySuperseded {
        /// The memory that holds the claim now.
        current_memory_id: MemoryId,
    },
    /// The workspace holds a different claim at this key, which the candidate replaces.
    Correction {
        /// The memory the candidate corrects.
        supersedes: MemoryId,
    },
}

impl MemoryCandidateComparison {
    /// Returns the stable name for logs and wire values.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Duplicate { .. } => "duplicate",
            Self::AlreadySuperseded { .. } => "already_superseded",
            Self::Correction { .. } => "correction",
        }
    }
}

/// Compares a content against the workspace's current claim at a candidate's key.
///
/// # Why this is a free function
///
/// `existing`, `content`, and `superseded_by` are exactly what the store answers, and the **rule** about
/// which answer means what lives here so a daemon's query and a storage layer's rows cannot disagree
/// about whether a shared key means "reinforce" or "supersede". A test's fixture then runs this same
/// function rather than a second reading of one sentence.
///
/// # How each outcome arises, including the one a correct caller cannot reach
///
/// - **`New`** — the store has no current memory at the key and no replacement. The ordinary case.
/// - **`Duplicate`** — the store's current memory at the key holds the same words. The expected outcome
///   whenever `existing` came from a lookup by the candidate's own key, because the key *is* the sorted
///   word set: a key match with different words is not possible.
/// - **`AlreadySuperseded`** — no current memory at the key, but something replaced the claim there. The
///   workspace holds this claim and has retired it: reinforcing the replacement would count a retrieval
///   the candidate did not cause, and writing a row would resurrect a claim the current one replaced.
/// - **`Correction`** — `existing` holds *different* words. **A correct caller cannot produce this**, since
///   its own key lookup would not have returned that row. It is here because a caller supplies `existing`,
///   and a caller that supplied a row from a *wrong* lookup would otherwise have its row reinforced as if
///   it held the candidate's claim. So the branch is the symptom of a key-mismatch bug, and returning it
///   rather than `Duplicate` is what makes that bug produce a supersession instead of a silent merge.
#[must_use]
pub fn compare(
    existing: Option<&MemoryRecord>,
    content: &str,
    superseded_by: Option<MemoryId>,
) -> MemoryCandidateComparison {
    match existing {
        Some(current) => {
            if normalized_equal(current.content(), content) {
                MemoryCandidateComparison::Duplicate {
                    existing_memory_id: current.id(),
                }
            } else {
                MemoryCandidateComparison::Correction {
                    supersedes: current.id(),
                }
            }
        }
        None => match superseded_by {
            Some(current_memory_id) => {
                MemoryCandidateComparison::AlreadySuperseded { current_memory_id }
            }
            None => MemoryCandidateComparison::New,
        },
    }
}

/// Compares two contents the way the search key does: case-folded, whitespace-collapsed, word-ordered.
///
/// # Why this is not `==`
///
/// The search key's normalization is the store's definition of "the same claim", so a comparison using
/// raw equality would report a **correction** for a candidate differing only in spacing — and a
/// correction supersedes a memory, retiring a claim for a formatting difference. Sharing the
/// normalization is what keeps "duplicate" and "same key" the same relation.
///
/// Case folding is **ASCII only**, matching the key's own rule: `to_lowercase` on non-ASCII can change a
/// string's length and is locale-dependent, and the key's normalization deliberately avoids both.
#[must_use]
pub fn normalized_equal(left: &str, right: &str) -> bool {
    let mut left_words = normalized_words(left);
    let mut right_words = normalized_words(right);
    if left_words.len() != right_words.len() {
        return false;
    }
    left_words.sort_unstable();
    right_words.sort_unstable();
    left_words == right_words
}

/// Splits content into words, whitespace-collapsed and ASCII-case-folded.
fn normalized_words(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect()
}

#[cfg(test)]
#[path = "candidate/tests.rs"]
mod tests;
