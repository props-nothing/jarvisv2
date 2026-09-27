//! Typed durable memory: what JARVIS may remember, how it is sourced, and when it stops being true.
//!
//! `docs/architecture/memory-and-context.md` states the rule this module implements in one sentence: "A
//! memory is a sourced claim with lifecycle metadata, not an unqualified string." Everything here follows
//! from taking that literally.
//!
//! # Why the source is a required field rather than a string
//!
//! The document's admission lifecycle has a **rejection** step: a candidate "not supported by source" is
//! not persisted as fact. A type that could hold content without a source would make that step
//! unenforceable, because there would be no field to check — so [`MemorySource`] is not optional and
//! [`MemoryRecord::new`] refuses to build a record without one. The consequence is that "an inferred
//! preference cannot appear as confirmed fact" is a property of the constructor rather than a rule a
//! reader has to remember.
//!
//! # Why confidence is not a float
//!
//! A `f64` confidence invites arithmetic that means nothing (two confidences averaged are not a third
//! confidence) and it serialises differently across platforms. [`MemoryConfidence`] is a small ordered set
//! with a **named** level, so a stored value that this build does not recognise is reported rather than
//! silently becoming `0.0` — which would read as "no confidence" and quietly demote a trusted claim.
//!
//! # Why `status` is stored and validity is derived
//!
//! The same split `jarvis_core::ApprovalState` makes, for the same reason. A memory has a **stored**
//! status — a human archived it, a correction superseded it, a policy deleted it — and an **effective**
//! state that also depends on `valid_until` and on the current clock. Storing `Expired` would require a
//! sweep job for correctness and would make a row restored from a backup wrong.
//!
//! # What this module deliberately does not do
//!
//! - **It does not rank or retrieve.** Eligibility (workspace, sensitivity, status, source trust) and
//!   ranking are `P4-004`; this module supplies the vocabulary both operate on.
//! - **It does not resolve entities.** [`EntityRef`] names an entity and how confidently it was matched;
//!   *merging* two entities is an auditable, reversible operation with its own slice, and the document is
//!   explicit that similarity alone must never drive it.
//! - **It does not embed anything.** The embedding columns in the canonical record are `P4-005`, and a
//!   memory must be fully usable without them — "embeddings are one signal", not a requirement.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::id::{EntityId, MemoryId, RunId, WorkspaceId};
use crate::sensitivity::Sensitivity;
use crate::timestamp::UtcTimestamp;

/// The longest useful memory text, in characters.
///
/// A bound rather than a preference: a memory is context that will be *selected* into a prompt, so an
/// unbounded one is a single record that can consume an entire budget. Truncation is refused rather than
/// applied, because a silently shortened claim is a different claim.
pub const MAX_MEMORY_CONTENT_CHARS: usize = 4096;

/// The longest structured claim document, in bytes.
///
/// Bounded in **bytes** because that is what a transport enforces, and the check is on the serialized
/// form so a claim cannot be small as a value and large on the wire — the same reasoning
/// `jarvis_core::RunEventPayload` uses.
pub const MAX_STRUCTURED_CLAIM_BYTES: usize = 8192;

/// The most entities one memory may be about.
///
/// A claim about everything is a claim about nothing, and a bound keeps "find the memories about X" from
/// becoming a scan. The number is generous for the kinds the architecture lists (a person, an
/// organization, a project, a document) while still refusing a document-sized list.
pub const MAX_MEMORY_ENTITIES: usize = 16;

/// The most words a search key may contain.
pub const MAX_SEARCH_KEY_WORDS: usize = 32;

/// The longest one search key word, in characters.
pub const MAX_SEARCH_KEY_WORD_CHARS: usize = 64;

/// The longest a normalized search key may be, in characters.
pub const MAX_SEARCH_KEY_CHARS: usize = 512;

/// The longest a predicate or object may be, in characters.
pub const MAX_CLAIM_PART_CHARS: usize = 256;

/// The longest a source locator may be, in characters.
///
/// Bounded because a locator is stored, indexed, and returned to a user asking "why was this remembered",
/// and because an unbounded one is a place to hide a payload. It is **not** a path — see
/// [`MemorySource`] on why no filesystem path is ever stored here.
pub const MAX_SOURCE_LOCATOR_CHARS: usize = 512;

/// Explains why a memory field was refused.
///
/// One variant per rule, carrying the field name where the shape is wrong, so a caller learns which
/// check failed without the message forwarding the content it rejected. A preview of rejected content in
/// an error is how a rejected secret ends up in a log.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidMemory {
    /// The content was empty, whitespace-only, or longer than [`MAX_MEMORY_CONTENT_CHARS`].
    #[error("memory content must be non-empty and at most {MAX_MEMORY_CONTENT_CHARS} characters")]
    Content,
    /// The structured claim did not serialise to a JSON object within [`MAX_STRUCTURED_CLAIM_BYTES`].
    #[error(
        "a structured claim must be a JSON object of at most {MAX_STRUCTURED_CLAIM_BYTES} bytes"
    )]
    StructuredClaim,
    /// The source was unusable: an empty or oversized locator, or a kind/text mismatch.
    #[error("the memory source is unusable")]
    Source,
    /// A source excerpt hash was not a SHA-256 digest.
    #[error("a source excerpt hash must be 64 lowercase hexadecimal characters")]
    SourceExcerptHash,
    /// The entity list was empty after deduplication, or longer than [`MAX_MEMORY_ENTITIES`].
    #[error("a memory must name between 1 and {MAX_MEMORY_ENTITIES} distinct entities")]
    Entities,
    /// A search key was malformed.
    #[error(
        "a search key must be 1 to {MAX_SEARCH_KEY_WORDS} words of at most \
             {MAX_SEARCH_KEY_WORD_CHARS} characters each"
    )]
    SearchKey,
    /// A structured claim part was empty or oversized.
    #[error("a claim part must be non-empty and at most {MAX_CLAIM_PART_CHARS} characters")]
    ClaimPart,
    /// The validity window was inverted, or did not follow the creation time.
    #[error(
        "a memory's valid_until must be after its valid_from, which must not precede its creation"
    )]
    Validity,
    /// The confidence text did not name a known level.
    #[error("confidence must be one of unverified, uncertain, likely, confirmed")]
    UnknownConfidence,
    /// The memory type text did not name a known type.
    #[error(
        "memory type must be one of working, conversation, episodic, semantic, preference, \
             relationship, procedural"
    )]
    UnknownType,
    /// The status text did not name a known status.
    #[error("memory status must be one of proposed, active, archived, deleted")]
    UnknownStatus,
    /// The source kind text did not name a known kind.
    #[error("source kind is not one this build recognizes")]
    UnknownSourceKind,
    /// The source trust text did not name a known class.
    #[error("source trust is not one this build recognizes")]
    UnknownTrust,
    /// An entity reference named no entity, or carried an unusable match basis.
    #[error("an entity reference is unusable")]
    EntityReference,
    /// A correction did not name the memory it supersedes.
    #[error("a correction must name the memory it supersedes")]
    SupersedesMissing,
    /// A supersession pointed at the memory itself.
    #[error("a memory must not supersede itself")]
    SupersedesSelf,
    /// The record was already deleted, and a deleted memory is terminal.
    #[error("a deleted memory cannot be changed")]
    AlreadyDeleted,
    /// The status a caller asked for is not reachable from the stored one.
    #[error("the status change from {from} to {to} is not permitted")]
    IllegalStatus {
        /// The stored status.
        from: &'static str,
        /// The requested status.
        to: &'static str,
    },
}

/// What a memory is for, and how long it is expected to matter.
///
/// The set is `docs/architecture/memory-and-context.md`'s own table. Two entries in that table are
/// deliberately **absent** and the reason is in the document: runtime checkpoints and provider
/// conversation IDs "are not memory types. They are opaque execution bindings." A type that could hold a
/// checkpoint would make the memory store a resume mechanism, and deleting a memory would then be a way
/// to break a running conversation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// The current objective, plan, observations, and pending calls. One run's lifetime.
    Working,
    /// Dialogue continuity and summaries. Session retention applies.
    Conversation,
    /// What happened and when. Long-lived and decay-aware.
    Episodic,
    /// Durable facts and concepts, until corrected, expired, or deleted.
    Semantic,
    /// How the user wants things presented or done. User-editable.
    Preference,
    /// People and organizations, with user-specific relationship context.
    Relationship,
    /// A reusable way of carrying out a task. Versioned and reviewable.
    Procedural,
}

impl MemoryType {
    /// Returns the stable storage and wire name.
    ///
    /// Written out rather than derived from `Debug`, so a variant rename cannot silently change what a
    /// stored row says — the rule every other vocabulary type in this crate follows.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Conversation => "conversation",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Preference => "preference",
            Self::Relationship => "relationship",
            Self::Procedural => "procedural",
        }
    }

    /// Returns every type, for a storage `CHECK` or a round-trip test to enumerate.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::Working,
            Self::Conversation,
            Self::Episodic,
            Self::Semantic,
            Self::Preference,
            Self::Relationship,
            Self::Procedural,
        ]
    }

    /// Returns whether this type is expected to survive the run that produced it.
    ///
    /// The distinction is load-bearing for retention rather than cosmetic. A `Working` memory is scoped to
    /// one run and a `Conversation` memory to a session, so both are *expected* to become collectable
    /// without anyone asking; the rest are durable until corrected or deleted. A retention policy that
    /// treated all seven alike would either keep per-run scratch forever or expire a confirmed preference.
    #[must_use]
    pub const fn is_durable(self) -> bool {
        match self {
            Self::Working | Self::Conversation => false,
            Self::Episodic
            | Self::Semantic
            | Self::Preference
            | Self::Relationship
            | Self::Procedural => true,
        }
    }

    /// Returns whether this type may require explicit user confirmation before becoming trusted.
    ///
    /// The document names the classes that do: "High-impact identity, medical, financial, authentication,
    /// and relationship inferences require explicit user confirmation before becoming trusted facts." Of
    /// the types here, `Relationship` is the one that carries that class, and it is why the flag lives on
    /// the type rather than on the caller — a caller that could decline to confirm would make the rule a
    /// convention.
    ///
    /// `Semantic` is **not** flagged, and that is deliberate rather than an omission: a semantic claim can
    /// be a medical or financial fact, and this build cannot tell from the type alone. So the flag means
    /// "this type is always high-impact", and the classification of an individual `Semantic` claim is
    /// `P4-003`'s job, where the content is actually read. Flagging the whole type here would refuse an
    /// ordinary fact about a city.
    #[must_use]
    pub const fn requires_confirmation(self) -> bool {
        matches!(self, Self::Relationship)
    }
}

impl fmt::Display for MemoryType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryType {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "working" => Ok(Self::Working),
            "conversation" => Ok(Self::Conversation),
            "episodic" => Ok(Self::Episodic),
            "semantic" => Ok(Self::Semantic),
            "preference" => Ok(Self::Preference),
            "relationship" => Ok(Self::Relationship),
            "procedural" => Ok(Self::Procedural),
            _ => Err(InvalidMemory::UnknownType),
        }
    }
}

/// How strongly a claim is supported, in named levels.
///
/// # Why four levels and not a number
///
/// The levels answer the question a caller actually has — may this be stated as fact? — and they answer
/// it the same way in every build. A float would let a caller threshold at `0.6` and have the meaning
/// change with a model upgrade, which is precisely the kind of silent drift this crate's vocabularies
/// exist to prevent.
///
/// The ordering is meaningful: a larger level is more support. It is used for a **floor** comparison
/// (a use case may require "at least `Likely`"), never for arithmetic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryConfidence {
    /// Nothing supports it beyond having been produced. **Never a fact.**
    Unverified,
    /// Partially supported, or inferred from an indirect signal.
    Uncertain,
    /// Well supported by a source that could be wrong.
    Likely,
    /// Stated or confirmed by a source authoritative for this claim.
    Confirmed,
}

impl MemoryConfidence {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Uncertain => "uncertain",
            Self::Likely => "likely",
            Self::Confirmed => "confirmed",
        }
    }

    /// Returns the ordered level, where a larger value is more support.
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::Unverified => 0,
            Self::Uncertain => 1,
            Self::Likely => 2,
            Self::Confirmed => 3,
        }
    }

    /// Returns every level, in ascending order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Unverified,
            Self::Uncertain,
            Self::Likely,
            Self::Confirmed,
        ]
    }

    /// Returns whether a claim at this level may be presented to a user as established.
    ///
    /// The single predicate the acceptance invariant turns on — "an inferred preference never appears as
    /// confirmed fact" — so it lives here rather than at a presentation site, where a caller could forget
    /// it. `Confirmed` is the only level that may be stated without hedging; `Likely` must be phrased as
    /// uncertain and `Unverified` must not be presented at all.
    #[must_use]
    pub const fn is_stated_as_fact(self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

impl fmt::Display for MemoryConfidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryConfidence {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "unverified" => Ok(Self::Unverified),
            "uncertain" => Ok(Self::Uncertain),
            "likely" => Ok(Self::Likely),
            "confirmed" => Ok(Self::Confirmed),
            _ => Err(InvalidMemory::UnknownConfidence),
        }
    }
}

/// Where a memory stands: what a human or policy has done to it, not what the clock says.
///
/// # Why this is four states and not five
///
/// `Expired` is deliberately **absent**, matching `approvals`. Expiry is a function of `valid_until` and
/// the current instant, so storing it would require a sweep job for correctness and would make a row
/// restored from a backup report the wrong state. [`MemoryStatus::effective_at`] derives the answer.
///
/// `Proposed` is present, and it is the state that makes the admission lifecycle real: a candidate that
/// deterministic code cannot support as fact is stored as a **proposal** rather than dropped, so a user
/// can review it. Dropping it would make "the model may propose candidates" produce nothing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Awaiting review. Retrieved only for a use case that asks for proposals.
    Proposed,
    /// Current and eligible for retrieval.
    Active,
    /// Superseded or set aside: retained for audit, not retrieved as current truth.
    Archived,
    /// Content removed. **Terminal**, and reached only through the deletion path.
    Deleted,
}

impl MemoryStatus {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Deleted => "deleted",
        }
    }

    /// Returns every status, for a round-trip test or a storage `CHECK`.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [Self::Proposed, Self::Active, Self::Archived, Self::Deleted]
    }

    /// Returns the state at an instant, taking the validity window into account.
    ///
    /// `Deleted` and `Proposed` are returned unchanged: a deleted memory is terminal whatever the clock
    /// says, and a proposal is a proposal until someone acts on it — expiring a proposal would silently
    /// discard a candidate nobody reviewed, which is the outcome `Proposed` exists to prevent.
    ///
    /// `Active` becomes [`EffectiveMemoryStatus::Expired`] once `valid_until` has passed, and `Archived`
    /// becomes [`EffectiveMemoryStatus::Superseded`] — the distinction matters because the two are
    /// different answers to "why is this not being used": one lapsed, one was replaced.
    #[must_use]
    pub const fn effective_at(
        self,
        now: UtcTimestamp,
        valid_until: Option<UtcTimestamp>,
    ) -> EffectiveMemoryStatus {
        match self {
            Self::Proposed => EffectiveMemoryStatus::Proposed,
            Self::Deleted => EffectiveMemoryStatus::Deleted,
            Self::Archived => EffectiveMemoryStatus::Superseded,
            Self::Active => match valid_until {
                Some(until) if now.unix_nanos() >= until.unix_nanos() => {
                    EffectiveMemoryStatus::Expired
                }
                _ => EffectiveMemoryStatus::Active,
            },
        }
    }

    /// Returns whether this status is a current claim that may be stated as truth.
    ///
    /// `true` for `Active` **only**. Stated on the stored status rather than the effective one because a
    /// caller holding a status and no clock is asking "is this the kind of thing that is current", and the
    /// time-dependent answer is [`Self::effective_at`]'s.
    #[must_use]
    pub const fn is_current_claim(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns whether this status is terminal.
    ///
    /// Only `Deleted`. `Archived` is deliberately not terminal: an archived memory can be restored, which
    /// is what makes the correction trail reversible rather than a one-way door.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Deleted)
    }
}

impl fmt::Display for MemoryStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryStatus {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "proposed" => Ok(Self::Proposed),
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            "deleted" => Ok(Self::Deleted),
            _ => Err(InvalidMemory::UnknownStatus),
        }
    }
}

/// What a memory's stored status means at a chosen instant.
///
/// Separate from [`MemoryStatus`] because the two answer different questions and a caller that conflated
/// them would store a time-dependent value. The same split `jarvis_core::ApprovalState` and its own
/// effective state make.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EffectiveMemoryStatus {
    /// Awaiting review.
    Proposed,
    /// Current and eligible.
    Active,
    /// Replaced by a correction, or set aside. Retained for audit.
    Superseded,
    /// Its validity window lapsed.
    Expired,
    /// Content removed. Terminal.
    Deleted,
}

impl EffectiveMemoryStatus {
    /// Returns the stable name, for a wire reply or an explanation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Deleted => "deleted",
        }
    }

    /// Returns whether a retrieved memory in this state may be presented as current truth.
    ///
    /// Only `Active`. The predicate `memory-and-context.md`'s first acceptance invariant needs: "a
    /// confirmed preference survives restart with provenance" is only satisfiable if a superseded or
    /// expired claim is not returned as though it were current.
    #[must_use]
    pub const fn is_current_truth(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl fmt::Display for EffectiveMemoryStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What kind of thing a memory came from.
///
/// # Why this is a closed set with its own trust class
///
/// The document lists source classes and adds the rule that decides their use: "Trust is not a single
/// global score. A provider may be authoritative for an event timestamp but not for a person's
/// preference." So the kind and the trust are **two fields**, and [`MemorySourceKind::permitted_trust`]
/// constrains which trust classes each kind may claim. That constraint is what makes "external untrusted
/// content" unable to masquerade as an authenticated provider record: a source claiming
/// `Authoritative` for a kind that cannot carry it is refused at construction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceKind {
    /// The user said it directly, in their own words.
    UserStatement,
    /// The user corrected or confirmed an existing claim.
    UserCorrection,
    /// An authenticated provider record: a calendar entry, a message the provider signed.
    ProviderRecord,
    /// A deterministic observation by a tool this platform ran.
    ToolObservation,
    /// An imported document or file.
    Document,
    /// The model inferred it.
    ModelInference,
    /// Content retrieved from outside the trust boundary: a web page, an email body.
    ExternalContent,
}

impl MemorySourceKind {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserStatement => "user_statement",
            Self::UserCorrection => "user_correction",
            Self::ProviderRecord => "provider_record",
            Self::ToolObservation => "tool_observation",
            Self::Document => "document",
            Self::ModelInference => "model_inference",
            Self::ExternalContent => "external_content",
        }
    }

    /// Returns every kind, for a round-trip test or a storage `CHECK`.
    #[must_use]
    pub const fn all() -> [Self; 7] {
        [
            Self::UserStatement,
            Self::UserCorrection,
            Self::ProviderRecord,
            Self::ToolObservation,
            Self::Document,
            Self::ModelInference,
            Self::ExternalContent,
        ]
    }

    /// Returns the trust class this kind may carry.
    ///
    /// # The assignments, and why each is what it is
    ///
    /// - `UserStatement` and `UserCorrection` are [`MemoryTrust::Authoritative`]: the person is the
    ///   authority on their own preferences, and a correction is by definition the authority overriding
    ///   something else.
    /// - `ProviderRecord` is `Authoritative`, and this is where the document's warning bites: a provider
    ///   is authoritative for **what it recorded**, so a claim from a calendar is authoritative about the
    ///   event's time. It is *not* authoritative about a person's preference, and that distinction is kept
    ///   by [`MemoryRecord::new`] refusing a provider-sourced `Preference` memory — a rule stated once, at
    ///   the constructor, rather than left to a reader.
    /// - `ToolObservation` is [`MemoryTrust::Derived`]: a tool's output is evidence this platform
    ///   gathered, which is stronger than an inference but is not a person speaking.
    /// - `Document` and `ModelInference` are `Derived`: both are things a process produced by reading
    ///   something else, and neither may instruct.
    /// - `ExternalContent` is [`MemoryTrust::Untrusted`], always. This is the injection boundary: content
    ///   from outside cannot be authoritative whatever it says about itself, and `memory-and-context.md`
    ///   requires it be "marked as untrusted data".
    #[must_use]
    pub const fn permitted_trust(self) -> MemoryTrust {
        match self {
            Self::UserStatement | Self::UserCorrection | Self::ProviderRecord => {
                MemoryTrust::Authoritative
            }
            Self::ToolObservation | Self::Document | Self::ModelInference => MemoryTrust::Derived,
            Self::ExternalContent => MemoryTrust::Untrusted,
        }
    }

    /// Returns whether this kind is something the **model** produced rather than something observed.
    ///
    /// Used to keep an inference from being promoted past [`MemoryConfidence::Unverified`] without a second
    /// source. A model cannot raise its own confidence — that would make "the model may propose candidates"
    /// a way to assert anything — so a `ModelInference` at a higher level is refused by
    /// [`MemoryRecord::new`].
    #[must_use]
    pub const fn is_model_produced(self) -> bool {
        matches!(self, Self::ModelInference)
    }
}

impl fmt::Display for MemorySourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemorySourceKind {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "user_statement" => Ok(Self::UserStatement),
            "user_correction" => Ok(Self::UserCorrection),
            "provider_record" => Ok(Self::ProviderRecord),
            "tool_observation" => Ok(Self::ToolObservation),
            "document" => Ok(Self::Document),
            "model_inference" => Ok(Self::ModelInference),
            "external_content" => Ok(Self::ExternalContent),
            _ => Err(InvalidMemory::UnknownSourceKind),
        }
    }
}

impl Default for MemorySourceKind {
    /// The safest kind: content from outside the trust boundary.
    ///
    /// A default that failed **closed** matters here because the default is what a deserialiser fills in
    /// when a field is absent, and a memory whose origin was never recorded must not be treated as
    /// something the user said.
    fn default() -> Self {
        Self::ExternalContent
    }
}

/// How much a source's claims may be trusted.
///
/// Ordered: a larger value is more trust. Used as a **floor** for a use case that demands a minimum
/// ("only user-confirmed preferences"), never as a score to average.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTrust {
    /// Content from outside the trust boundary. Data, never instruction.
    ///
    /// The default, for the same fail-closed reason [`MemorySourceKind::default`] gives.
    #[default]
    Untrusted,
    /// Produced by a process that read something else: a document parse, a model inference, a tool.
    Derived,
    /// The user, or a provider speaking about what it recorded.
    Authoritative,
}

impl MemoryTrust {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::Derived => "derived",
            Self::Authoritative => "authoritative",
        }
    }

    /// Returns the ordered level, where a larger value is more trust.
    #[must_use]
    pub const fn level(self) -> u8 {
        match self {
            Self::Untrusted => 0,
            Self::Derived => 1,
            Self::Authoritative => 2,
        }
    }

    /// Returns whether content at this trust class may carry instructions.
    ///
    /// Only `Authoritative`, which is the same rule `jarvis_core::assemble_context` enforces for prompt
    /// assembly. Stating it here as well is deliberate: a memory that reached retrieval with an
    /// instruction in its text and an `Untrusted` trust is data, and the context assembler would refuse it
    /// as instruction-bearing — so the two layers agree rather than one relying on the other.
    #[must_use]
    pub const fn may_instruct(self) -> bool {
        matches!(self, Self::Authoritative)
    }
}

impl fmt::Display for MemoryTrust {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryTrust {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "untrusted" => Ok(Self::Untrusted),
            "derived" => Ok(Self::Derived),
            "authoritative" => Ok(Self::Authoritative),
            _ => Err(InvalidMemory::UnknownTrust),
        }
    }
}

/// Where a claim came from: the class, the addressable origin, and the trust it may carry.
///
/// # Why the excerpt is a hash and not the excerpt
///
/// `source_excerpt_hash` in the canonical record is what links a claim to the text that supports it, and
/// the document is explicit that "hashes do not replace source retention rules" — the original stays
/// separately addressable through [`Self::locator`]. Storing the excerpt itself would put a second copy of
/// possibly-sensitive source text in the memory store, where deletion of the source would not reach it.
///
/// # Why no filesystem path is stored
///
/// A locator is bounded text naming where the source lives in **its own** system: a session and event
/// identifier, a provider's message identifier, a document identifier. Storing a path would make a memory
/// row a directory listing of the machine, and would break the moment the file moved — an addressable
/// identifier survives that.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemorySource {
    kind: MemorySourceKind,
    locator: String,
    trust: MemoryTrust,
    excerpt_hash: Option<String>,
}

impl MemorySource {
    /// Builds a source, refusing a kind/trust mismatch and an unusable locator.
    ///
    /// # Errors
    ///
    /// - [`InvalidMemory::Source`] for an empty or oversized locator, or a trust class the kind cannot
    ///   carry. **The trust check is the one that matters**: it is what makes `ExternalContent` unable to
    ///   claim `Authoritative`, so the injection boundary is a property of construction rather than of
    ///   every reader.
    /// - [`InvalidMemory::SourceExcerptHash`] when a supplied hash is not a lowercase SHA-256 digest.
    pub fn new(
        kind: MemorySourceKind,
        locator: impl Into<String>,
        trust: MemoryTrust,
        excerpt_hash: Option<String>,
    ) -> Result<Self, InvalidMemory> {
        let locator = locator.into();
        let trimmed = locator.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_SOURCE_LOCATOR_CHARS {
            return Err(InvalidMemory::Source);
        }
        // A kind has exactly one permitted trust, so this is equality rather than an ordering: a provider
        // record is authoritative and a tool observation is derived, and neither may claim the other's
        // class. An ordering would let a document claim `Authoritative`, which is the confusion the
        // two-field design exists to prevent.
        if kind.permitted_trust() != trust {
            return Err(InvalidMemory::Source);
        }
        let excerpt_hash = match excerpt_hash {
            Some(hash) => {
                if !is_sha256_hex(&hash) {
                    return Err(InvalidMemory::SourceExcerptHash);
                }
                Some(hash)
            }
            None => None,
        };
        Ok(Self {
            kind,
            locator: trimmed.to_owned(),
            trust,
            excerpt_hash,
        })
    }

    /// Builds a source of the kind's own permitted trust.
    ///
    /// The common case, and the one a caller should reach for: naming the class twice — once as the kind
    /// and once as the trust — is a chance to disagree, and this constructor cannot.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::Source`] for an unusable locator.
    pub fn of_kind(
        kind: MemorySourceKind,
        locator: impl Into<String>,
    ) -> Result<Self, InvalidMemory> {
        Self::new(kind, locator, kind.permitted_trust(), None)
    }

    /// Attaches an excerpt hash, which is the link to the supporting text.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::SourceExcerptHash`] when the hash is not a lowercase SHA-256 digest.
    pub fn with_excerpt_hash(mut self, hash: impl Into<String>) -> Result<Self, InvalidMemory> {
        let hash = hash.into();
        if !is_sha256_hex(&hash) {
            return Err(InvalidMemory::SourceExcerptHash);
        }
        self.excerpt_hash = Some(hash);
        Ok(self)
    }

    /// Returns the source class.
    #[must_use]
    pub const fn kind(&self) -> MemorySourceKind {
        self.kind
    }

    /// Returns the addressable origin, in the source's own system.
    #[must_use]
    pub fn locator(&self) -> &str {
        &self.locator
    }

    /// Returns the trust class this source may carry.
    #[must_use]
    pub const fn trust(&self) -> MemoryTrust {
        self.trust
    }

    /// Returns the digest of the supporting text, when one was recorded.
    #[must_use]
    pub fn excerpt_hash(&self) -> Option<&str> {
        self.excerpt_hash.as_deref()
    }
}

/// A normalized, order-independent key derived from a memory's content.
///
/// # Why this exists at all
///
/// The idempotency ledger for memory, and it is not the content: two memories stating one thing in
/// different words must **not** collapse, because the architecture says a correction creates "a new claim,
/// link `supersedes`" rather than editing. What must collapse is a genuine re-derivation of the *same*
/// text — the same tool observation stored twice, the same message ingested on two passes. So the key is
/// `type + entities + normalized words`, which is stable across a re-ingest and different for a reworded
/// claim.
///
/// # Why the normalization is this narrow
///
/// Case, whitespace, and word order, and nothing else. No stemming, no stop-word removal, no synonym
/// mapping: each of those would make two *different* claims collide, and a collision means a user's real
/// second statement is silently dropped as a duplicate. Under-collapsing a duplicate costs a row;
/// over-collapsing a distinct claim loses information the user gave.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MemorySearchKey(String);

impl MemorySearchKey {
    /// Builds a key from a memory's type, entities, and content.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::SearchKey`] when the content yields no usable word, or yields more than
    /// [`MAX_SEARCH_KEY_WORDS`] distinct words.
    pub fn new(
        memory_type: MemoryType,
        entities: &[EntityRef],
        content: &str,
    ) -> Result<Self, InvalidMemory> {
        let mut words: Vec<String> = Vec::new();
        for word in content.split_whitespace() {
            let normalized: String = word
                .chars()
                .filter(|character| character.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            if normalized.is_empty() || normalized.chars().count() > MAX_SEARCH_KEY_WORD_CHARS {
                continue;
            }
            if !words.contains(&normalized) {
                words.push(normalized);
            }
        }
        if words.is_empty() {
            return Err(InvalidMemory::SearchKey);
        }
        // Sorted so two orderings of one sentence produce one key. Bounded **after** sorting, so which
        // words survive does not depend on where they appeared.
        words.sort_unstable();
        if words.len() > MAX_SEARCH_KEY_WORDS {
            return Err(InvalidMemory::SearchKey);
        }

        let mut entity_ids: Vec<String> = entities
            .iter()
            .map(|entity| entity.entity_id().to_string())
            .collect();
        entity_ids.sort_unstable();
        entity_ids.dedup();

        let rendered = format!(
            "{}|{}|{}",
            memory_type.as_str(),
            entity_ids.join(","),
            words.join(" ")
        );
        if rendered.chars().count() > MAX_SEARCH_KEY_CHARS {
            return Err(InvalidMemory::SearchKey);
        }
        Ok(Self(rendered))
    }

    /// Returns the key as stored.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemorySearchKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// How confidently a memory's entity was matched to a canonical entity.
///
/// `memory-and-context.md` requires that entity resolution use "verified provider IDs, exact identifiers,
/// user confirmation, and probabilistic matches" and that "ambiguous aliases remain separate candidates".
/// This enum is what makes that auditable: a stored memory says *how* its subject was matched, so a later
/// reader can tell a confirmed identity from a guess without re-running resolution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityMatch {
    /// The user said so, or corrected it.
    Confirmed,
    /// Matched on an identifier the provider issued and this platform verified.
    ProviderId,
    /// Matched on an exact identifier: an email address, an account number, a document identifier.
    ExactIdentifier,
    /// Matched by a similarity signal. **The lowest confidence**, and the one that must never merge.
    Probabilistic,
}

impl EntityMatch {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::ProviderId => "provider_id",
            Self::ExactIdentifier => "exact_identifier",
            Self::Probabilistic => "probabilistic",
        }
    }

    /// Returns whether this match is strong enough to treat two mentions as one entity.
    ///
    /// `Probabilistic` is excluded, and that is the document's rule stated as a predicate: "Never merge
    /// solely because embeddings are similar." A caller that asked this before merging would get `false`
    /// for a similarity match, so a merge driven only by embeddings cannot be expressed as a call this
    /// build accepts.
    #[must_use]
    pub const fn may_merge(self) -> bool {
        matches!(
            self,
            Self::Confirmed | Self::ProviderId | Self::ExactIdentifier
        )
    }
}

impl std::str::FromStr for EntityMatch {
    type Err = InvalidMemory;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "confirmed" => Ok(Self::Confirmed),
            "provider_id" => Ok(Self::ProviderId),
            "exact_identifier" => Ok(Self::ExactIdentifier),
            "probabilistic" => Ok(Self::Probabilistic),
            _ => Err(InvalidMemory::EntityReference),
        }
    }
}

/// One entity a memory is about, with how confidently it was matched.
///
/// The `match_basis` travels with the identifier rather than being a property of the entity, because the
/// **same** entity may be matched confidently in one memory and guessed in another — and a reader deciding
/// whether a claim is safe to state needs the weaker of the two answers for the memory in hand.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityRef {
    entity_id: EntityId,
    matched_by: EntityMatch,
}

impl EntityRef {
    /// Names an entity and how it was matched.
    #[must_use]
    pub const fn new(entity_id: EntityId, matched_by: EntityMatch) -> Self {
        Self {
            entity_id,
            matched_by,
        }
    }

    /// Names an entity the user (or a provider) established.
    #[must_use]
    pub const fn confirmed(entity_id: EntityId) -> Self {
        Self::new(entity_id, EntityMatch::Confirmed)
    }

    /// Returns the canonical entity.
    #[must_use]
    pub const fn entity_id(&self) -> EntityId {
        self.entity_id
    }

    /// Returns how the entity was matched.
    #[must_use]
    pub const fn matched_by(&self) -> EntityMatch {
        self.matched_by
    }
}

/// A normalized subject/predicate/object claim, when one was derivable.
///
/// `content` is the useful text and this is the machine-readable form. Both are kept because they answer
/// different questions: text is what a user reads when asking "why do you think that", and the triple is
/// what makes "a preference about the same predicate" findable without parsing prose.
///
/// # Why the object is text rather than a typed value
///
/// A typed object would need an ontology this build does not have, and guessing one would make a stored
/// claim's meaning depend on the reader's schema version. Text with a bounded length is honest about what
/// is known; `P4-006`'s retrieval can compare it exactly or through full-text without pretending to
/// interpret it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuredClaim {
    subject: String,
    predicate: String,
    object: String,
}

impl StructuredClaim {
    /// Builds a claim, refusing an empty or oversized part.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::ClaimPart`] for a part that is empty or longer than
    /// [`MAX_CLAIM_PART_CHARS`].
    pub fn new(
        subject: impl Into<String>,
        predicate: impl Into<String>,
        object: impl Into<String>,
    ) -> Result<Self, InvalidMemory> {
        let part = |value: String| -> Result<String, InvalidMemory> {
            let trimmed = value.trim().to_owned();
            if trimmed.is_empty() || trimmed.chars().count() > MAX_CLAIM_PART_CHARS {
                return Err(InvalidMemory::ClaimPart);
            }
            Ok(trimmed)
        };
        Ok(Self {
            subject: part(subject.into())?,
            predicate: part(predicate.into())?,
            object: part(object.into())?,
        })
    }

    /// Returns the claim's subject.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Returns the claim's predicate.
    #[must_use]
    pub fn predicate(&self) -> &str {
        &self.predicate
    }

    /// Returns the claim's object.
    #[must_use]
    pub fn object(&self) -> &str {
        &self.object
    }

    /// Renders the canonical `subject|predicate|object` key.
    ///
    /// Separated from [`Self::new`] so the rendering is one function rather than several call sites
    /// building the same string. The delimiter is a character the parts cannot contain after trimming,
    /// because a part containing `|` would make two different claims render identically.
    #[must_use]
    pub fn as_key(&self) -> String {
        format!("{}|{}|{}", self.subject, self.predicate, self.object)
    }
}

/// The declared fields of a memory.
///
/// A struct rather than fifteen positional parameters, and without `Default`, so every field is stated at
/// the call site — the shape `ApprovalRequestParts` uses for the same reason.
#[derive(Clone, Debug)]
pub struct MemoryRecordParts {
    /// The memory identifier.
    pub id: MemoryId,
    /// The workspace that owns it, which is also the boundary retrieval may not cross.
    pub workspace_id: WorkspaceId,
    /// What kind of memory this is.
    pub memory_type: MemoryType,
    /// The useful text.
    pub content: String,
    /// The normalized claim, when one was derivable.
    pub structured_claim: Option<StructuredClaim>,
    /// Where it came from. **Required**: a claim with no source is not a memory.
    pub source: MemorySource,
    /// How well supported it is.
    pub confidence: MemoryConfidence,
    /// How much it matters, as a bounded rank.
    pub importance: u8,
    /// How widely it may be disclosed.
    pub sensitivity: Sensitivity,
    /// The entities it is about.
    pub entities: Vec<EntityRef>,
    /// When it becomes true. Absent means "as of its creation".
    pub valid_from: Option<UtcTimestamp>,
    /// When it stops being true. Absent means "until corrected".
    pub valid_until: Option<UtcTimestamp>,
    /// The memory this one replaces, for a correction.
    pub supersedes: Option<MemoryId>,
    /// The run that produced it, when one did.
    pub run_id: Option<RunId>,
    /// The actor that produced it.
    pub created_by_actor_id: String,
    /// The correlation identity shared with the originating request.
    pub correlation_id: crate::id::CorrelationId,
    /// When it was recorded.
    pub created_at: UtcTimestamp,
}

/// One durable memory: a sourced claim with lifecycle metadata.
///
/// Immutable after construction except through the four transitions this module exposes — `confirm`,
/// `correct`, `archive`, and `delete` — each of which is a named act rather than a field assignment, so
/// "how did this memory change" is answerable from the call sites that could have changed it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryRecord {
    id: MemoryId,
    workspace_id: WorkspaceId,
    memory_type: MemoryType,
    content: String,
    structured_claim: Option<StructuredClaim>,
    source: MemorySource,
    confidence: MemoryConfidence,
    importance: u8,
    sensitivity: Sensitivity,
    entities: Vec<EntityRef>,
    valid_from: UtcTimestamp,
    valid_until: Option<UtcTimestamp>,
    supersedes: Option<MemoryId>,
    superseded_by: Option<MemoryId>,
    status: MemoryStatus,
    run_id: Option<RunId>,
    created_by_actor_id: String,
    correlation_id: crate::id::CorrelationId,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
    /// When it was last selected into a context, for recency and decay.
    last_accessed_at: Option<UtcTimestamp>,
    /// How many times it has been usefully retrieved, which is the reinforcement signal.
    retrieval_count: u32,
}

impl MemoryRecord {
    /// Validates and records a memory.
    ///
    /// # Errors
    ///
    /// - [`InvalidMemory::Content`] for empty, whitespace-only, or oversized text.
    /// - [`InvalidMemory::Entities`] for no entity or more than [`MAX_MEMORY_ENTITIES`].
    /// - [`InvalidMemory::Validity`] for an inverted or impossible window.
    /// - [`InvalidMemory::SupersedesSelf`] for a self-reference.
    /// - [`InvalidMemory::Source`] when a **model inference** is presented at a confidence above
    ///   [`MemoryConfidence::Unverified`], which is the rule that stops a model asserting a fact.
    /// - [`InvalidMemory::Source`] when a **provider record** backs a [`MemoryType::Preference`], which is
    ///   the document's own example of trust that does not transfer between claim kinds.
    pub fn new(parts: MemoryRecordParts) -> Result<Self, InvalidMemory> {
        let MemoryRecordParts {
            id,
            workspace_id,
            memory_type,
            content,
            structured_claim,
            source,
            confidence,
            importance,
            sensitivity,
            entities,
            valid_from,
            valid_until,
            supersedes,
            run_id,
            created_by_actor_id,
            correlation_id,
            created_at,
        } = parts;

        let content = content.trim().to_owned();
        if content.is_empty() || content.chars().count() > MAX_MEMORY_CONTENT_CHARS {
            return Err(InvalidMemory::Content);
        }

        // Deduplicated by entity, keeping the **weakest** match basis for a repeated entity. Keeping the
        // strongest would let one confident mention launder a second guess about the same entity in the
        // same memory, and the reader of the memory cannot tell which mention it was.
        let mut entities: Vec<EntityRef> = dedupe_entities(entities);
        if entities.is_empty() || entities.len() > MAX_MEMORY_ENTITIES {
            return Err(InvalidMemory::Entities);
        }
        entities.sort_by_key(EntityRef::entity_id);

        if let Some(superseded) = &supersedes
            && *superseded == id
        {
            return Err(InvalidMemory::SupersedesSelf);
        }

        // A model inference is a proposal and nothing more. Refusing a higher confidence here is what
        // makes "never persist unsupported inference as fact" a constructor rule rather than a review
        // note: there is no way to build the record the rule forbids.
        if source.kind().is_model_produced() && confidence != MemoryConfidence::Unverified {
            return Err(InvalidMemory::Source);
        }
        // A provider is authoritative about what it recorded and not about a person's preference. Stated
        // here, once, rather than left for each reader to apply.
        if source.kind() == MemorySourceKind::ProviderRecord
            && memory_type == MemoryType::Preference
        {
            return Err(InvalidMemory::Source);
        }

        let valid_from = valid_from.unwrap_or(created_at);
        if let Some(until) = valid_until
            && until.unix_nanos() <= valid_from.unix_nanos()
        {
            return Err(InvalidMemory::Validity);
        }
        // The window may not start before the memory existed: a claim cannot have been true from a moment
        // before it was recorded unless the source said so, and a caller supplying an earlier instant is
        // backdating a fact — which is how a later correction would fail to outrank it.
        if valid_from.unix_nanos() < created_at.unix_nanos() {
            return Err(InvalidMemory::Validity);
        }

        // A relationship inference is high-impact, so it starts as a proposal rather than as a fact. The
        // status is derived from the type here rather than taken from the caller, so the rule is a property
        // of construction: a caller cannot record a `Relationship` claim already active.
        let status = if memory_type.requires_confirmation() {
            MemoryStatus::Proposed
        } else {
            MemoryStatus::Active
        };

        Ok(Self {
            id,
            workspace_id,
            memory_type,
            content,
            structured_claim,
            source,
            confidence,
            importance: importance.min(MAX_MEMORY_IMPORTANCE),
            sensitivity,
            entities,
            valid_from,
            valid_until,
            supersedes,
            superseded_by: None,
            status,
            run_id,
            created_by_actor_id,
            correlation_id,
            created_at,
            updated_at: created_at,
            last_accessed_at: None,
            retrieval_count: 0,
        })
    }

    /// Returns the memory identifier.
    #[must_use]
    pub const fn id(&self) -> MemoryId {
        self.id
    }

    /// Returns the owning workspace, which is the retrieval boundary.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns the memory type.
    #[must_use]
    pub const fn memory_type(&self) -> MemoryType {
        self.memory_type
    }

    /// Returns the useful text.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns the normalized claim, when one was derivable.
    #[must_use]
    pub const fn structured_claim(&self) -> Option<&StructuredClaim> {
        self.structured_claim.as_ref()
    }

    /// Returns where the claim came from.
    #[must_use]
    pub const fn source(&self) -> &MemorySource {
        &self.source
    }

    /// Returns how well supported the claim is.
    #[must_use]
    pub const fn confidence(&self) -> MemoryConfidence {
        self.confidence
    }

    /// Returns the bounded importance rank.
    #[must_use]
    pub const fn importance(&self) -> u8 {
        self.importance
    }

    /// Returns how widely the content may be disclosed.
    #[must_use]
    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// Returns the entities the claim is about.
    #[must_use]
    pub fn entities(&self) -> &[EntityRef] {
        &self.entities
    }

    /// Returns when the claim becomes true.
    #[must_use]
    pub const fn valid_from(&self) -> UtcTimestamp {
        self.valid_from
    }

    /// Returns when the claim stops being true, when it does.
    #[must_use]
    pub const fn valid_until(&self) -> Option<UtcTimestamp> {
        self.valid_until
    }

    /// Returns the memory this one replaces.
    #[must_use]
    pub const fn supersedes(&self) -> Option<MemoryId> {
        self.supersedes
    }

    /// Returns the memory that replaced this one, once one has.
    ///
    /// Both directions are stored. `supersedes` alone would make "is this claim still current" a scan of
    /// every later memory, and the answer is needed on the retrieval path.
    #[must_use]
    pub const fn superseded_by(&self) -> Option<MemoryId> {
        self.superseded_by
    }

    /// Returns the stored status.
    #[must_use]
    pub const fn status(&self) -> MemoryStatus {
        self.status
    }

    /// Returns the status at an instant, taking validity into account.
    #[must_use]
    pub const fn effective_status_at(&self, now: UtcTimestamp) -> EffectiveMemoryStatus {
        self.status.effective_at(now, self.valid_until)
    }

    /// Returns the run that produced the memory, when one did.
    #[must_use]
    pub const fn run_id(&self) -> Option<RunId> {
        self.run_id
    }

    /// Returns the actor that produced the memory.
    #[must_use]
    pub fn created_by_actor_id(&self) -> &str {
        &self.created_by_actor_id
    }

    /// Returns the correlation identity shared with the originating request.
    #[must_use]
    pub const fn correlation_id(&self) -> crate::id::CorrelationId {
        self.correlation_id
    }

    /// Returns when the memory was recorded.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }

    /// Returns when the memory was last changed.
    #[must_use]
    pub const fn updated_at(&self) -> UtcTimestamp {
        self.updated_at
    }

    /// Returns when the memory was last selected into a context.
    #[must_use]
    pub const fn last_accessed_at(&self) -> Option<UtcTimestamp> {
        self.last_accessed_at
    }

    /// Returns how many times the memory has been usefully retrieved.
    #[must_use]
    pub const fn retrieval_count(&self) -> u32 {
        self.retrieval_count
    }

    /// Returns whether this claim may be offered to a user as established.
    ///
    /// Both conditions are needed and neither implies the other: a `Confirmed` claim that has been
    /// superseded is not current, and an active claim supported only `Likely` must still be hedged. The
    /// single predicate a presentation site should ask, so the two rules cannot be applied independently
    /// by a caller that remembers one of them.
    #[must_use]
    pub fn is_stateable_as_fact_at(&self, now: UtcTimestamp) -> bool {
        self.effective_status_at(now).is_current_truth() && self.confidence.is_stated_as_fact()
    }

    /// Returns whether this memory may be retrieved for the given workspace.
    ///
    /// The workspace check is an **equality**, not a containment or a hierarchy: `P4-004` owns the scoping
    /// rules, and a memory layer that invented an inheritance rule would be the layer that let one client's
    /// memory into another's context. Stated here so a retrieval implemented later has one place to ask.
    #[must_use]
    pub fn is_visible_in(&self, workspace_id: WorkspaceId) -> bool {
        self.workspace_id == workspace_id
    }

    /// Confirms a proposed memory, making it current.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::IllegalStatus`] from a status other than `Proposed`, and
    /// [`InvalidMemory::AlreadyDeleted`] for a deleted memory. A proposal is the only state that may be
    /// confirmed: re-confirming an active memory would bump its `updated_at` and make "when did this become
    /// trusted" unanswerable, and reviving a deleted one would defeat deletion.
    pub fn confirm(&self, at: UtcTimestamp) -> Result<Self, InvalidMemory> {
        self.transition(MemoryStatus::Active, at)
    }

    /// Archives a memory, retaining it for audit without retrieving it as current.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::IllegalStatus`] from `Deleted`, and [`InvalidMemory::AlreadyDeleted`] for a
    /// deleted memory. Archiving an already-archived memory is refused rather than treated as a no-op, so a
    /// caller learns it is acting on a stale read.
    pub fn archive(&self, at: UtcTimestamp) -> Result<Self, InvalidMemory> {
        self.transition(MemoryStatus::Archived, at)
    }

    /// Deletes a memory: the content is removed and the memory becomes terminal.
    ///
    /// # Why the content is cleared here as well as in storage
    ///
    /// `memory-and-context.md` requires that "deleting it removes text and derived indexes", and the
    /// acceptance invariant is "deleting it removes text and derived indexes". Clearing the field means a
    /// value already in memory cannot be returned by a caller that kept a reference, and it makes the
    /// domain — not only the repository — unable to produce the deleted text. The row's *existence* is
    /// retained so a source link and an audit record still resolve; the text is not.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::AlreadyDeleted`] for a memory already deleted. Deletion is terminal, so a
    /// second attempt is a caller acting on a stale read rather than a no-op.
    pub fn delete(&self, at: UtcTimestamp) -> Result<Self, InvalidMemory> {
        let deleted = self.transition(MemoryStatus::Deleted, at)?;
        Ok(Self {
            content: String::new(),
            structured_claim: None,
            superseded_by: None,
            ..deleted
        })
    }

    /// Records that another memory has replaced this one.
    ///
    /// # Why this is named for the act rather than mirroring the field
    ///
    /// An earlier shape called this `superseded_by`, which collided with the accessor of the same name and
    /// left the two distinguishable only by arity. A reader seeing `memory.superseded_by(..)` could not tell
    /// a transition from a query, and the transition is the one that changes state — so it takes the name
    /// that says it does something.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidMemory::AlreadyDeleted`] for a deleted memory, and
    /// [`InvalidMemory::SupersedesSelf`] for the memory's own identifier. Setting a replacement that is
    /// already set is refused unless it names the same memory, so a correction trail cannot be rewritten —
    /// which is what the "retain the correction trail" requirement needs.
    pub fn replace_with(
        &self,
        replacement: MemoryId,
        at: UtcTimestamp,
    ) -> Result<Self, InvalidMemory> {
        if self.status.is_terminal() {
            return Err(InvalidMemory::AlreadyDeleted);
        }
        if replacement == self.id {
            return Err(InvalidMemory::SupersedesSelf);
        }
        if let Some(existing) = self.superseded_by {
            if existing == replacement {
                return Ok(self.clone());
            }
            return Err(InvalidMemory::IllegalStatus {
                from: self.status.as_str(),
                to: self.status.as_str(),
            });
        }
        Ok(Self {
            superseded_by: Some(replacement),
            status: MemoryStatus::Archived,
            updated_at: at,
            ..self.clone()
        })
    }

    /// Records that this memory was selected into a context.
    ///
    /// The reinforcement signal `P4-004` ranks on. Separate from the lifecycle transitions because it is
    /// not a change to the claim — it is evidence that the claim was useful, and mixing it into
    /// [`Self::updated_at`] would make "when was this last edited" unanswerable.
    #[must_use]
    pub fn retrieved(&self, at: UtcTimestamp) -> Self {
        Self {
            last_accessed_at: Some(at),
            retrieval_count: self.retrieval_count.saturating_add(1),
            ..self.clone()
        }
    }

    /// Applies a status transition, refusing an illegal or repeated one.
    fn transition(&self, to: MemoryStatus, at: UtcTimestamp) -> Result<Self, InvalidMemory> {
        if self.status.is_terminal() {
            return Err(InvalidMemory::AlreadyDeleted);
        }
        if !self.status.can_advance_to(to) {
            return Err(InvalidMemory::IllegalStatus {
                from: self.status.as_str(),
                to: to.as_str(),
            });
        }
        Ok(Self {
            status: to,
            updated_at: at,
            ..self.clone()
        })
    }
}

impl MemoryStatus {
    /// Returns whether one status may follow another.
    ///
    /// # The table, and the edges that are deliberately absent
    ///
    /// ```text
    /// proposed -> active        a human or policy accepted the candidate
    /// proposed -> archived      set aside without ever becoming current
    /// active   -> archived      superseded, or set aside
    /// archived -> active        restored: the correction trail is reversible
    /// anything -> deleted       deletion is always available
    /// ```
    ///
    /// `active -> proposed` is absent: demoting a current claim to a proposal would make it disappear from
    /// retrieval without saying why, and "this was wrong" is a correction, not a demotion. `archived ->
    /// proposed` is absent for the same reason. `proposed -> proposed` and `active -> active` are absent so
    /// a repeated call is reported rather than silently bumping `updated_at`.
    #[must_use]
    pub const fn can_advance_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Proposed, Self::Active | Self::Archived)
                | (Self::Active, Self::Archived)
                | (Self::Archived, Self::Active)
                | (_, Self::Deleted)
        )
    }
}

/// The highest importance rank a memory may declare.
///
/// A small bounded rank rather than a score, so "how much does this matter" is answerable the same way in
/// every build and cannot become a number a model talks itself into.
pub const MAX_MEMORY_IMPORTANCE: u8 = 4;

/// Deduplicates entity references, keeping the **weakest** match basis for a repeated entity.
///
/// Weakest rather than strongest, and that direction is the safe one: a memory listing one entity twice —
/// once confidently from a provider identifier, once as a similarity guess — must be treated as the guess,
/// because the reader cannot tell which mention it is looking at. Keeping the strongest would let a
/// confident mention launder a later guess within one memory.
fn dedupe_entities(entities: Vec<EntityRef>) -> Vec<EntityRef> {
    let mut out: Vec<EntityRef> = Vec::with_capacity(entities.len());
    for entity in entities {
        if let Some(existing) = out
            .iter_mut()
            .find(|candidate| candidate.entity_id == entity.entity_id)
        {
            // `EntityMatch` is not ordered, so the weaker basis is named explicitly rather than compared.
            if weaker_match(entity.matched_by, existing.matched_by) == entity.matched_by {
                existing.matched_by = entity.matched_by;
            }
        } else {
            out.push(entity);
        }
    }
    out
}

/// Returns the weaker of two match bases.
///
/// A rank rather than an ordering on the enum, because the enum's declaration order is about how a match
/// was established and not about confidence — and a reordered variant must not silently change which basis
/// a memory ends up recorded with.
const fn weaker_match(left: EntityMatch, right: EntityMatch) -> EntityMatch {
    const fn rank(value: EntityMatch) -> u8 {
        match value {
            EntityMatch::Confirmed => 3,
            EntityMatch::ProviderId => 2,
            EntityMatch::ExactIdentifier => 1,
            EntityMatch::Probabilistic => 0,
        }
    }
    if rank(left) <= rank(right) {
        left
    } else {
        right
    }
}

/// Returns whether a value is exactly 64 lowercase hexadecimal characters.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests;
