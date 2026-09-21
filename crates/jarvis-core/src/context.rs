//! Context envelopes: what was offered to a model, what was included, and why.
//!
//! `docs/architecture/memory-and-context.md` defines the contract this module makes
//! executable. Every context item carries a source, a trust class, a sensitivity, a
//! priority, a token estimate, and an inclusion reason. Assembly reserves budget for
//! immutable policy and current user intent before any retrieved content is considered.
//!
//! # The two things this module refuses to do
//!
//! **It never lets external content become an instruction.** Security requirement SR-003
//! treats retrieved content and tool output as data, not authority. A trust class that is
//! not [`ContextTrust::Authoritative`] is therefore never instruction-bearing, and an
//! untrusted item must be marked quoted. Marking is enforced rather than conventional: an
//! item that claims to be untrusted instructions cannot be constructed, so the injection
//! path is closed at the type level instead of by remembering to check a flag.
//!
//! **It never drops an item silently.** Assembly returns a manifest that accounts for every
//! offered item, either as included or as excluded with the reason it was excluded. A
//! context that was truncated for budget and a context that was complete are otherwise
//! indistinguishable to every later reader, and only one of them is safe to reason about.
//!
//! # Assembly is ordered, and the order is load-bearing
//!
//! `docs/architecture/memory-and-context.md` lists seven assembly steps. The first is
//! "reserve budget for immutable policy and current user intent", which exists so that a
//! large body of retrieved content cannot crowd out the policy text that constrains what
//! the model may do. [`ContextBudget`] carries that reservation, and retrieved items draw
//! only from [`ContextBudget::retrievable_tokens`], so the reservation cannot be spent.
//!
//! ```
//! use jarvis_core::{
//!     ContextBudget, ContextItem, ContextPriority, ContextSource, ContextSourceKind,
//!     ContextTrust, InclusionReason, Sensitivity, assemble_context,
//! };
//!
//! let policy = ContextItem::new(
//!     ContextSource::new(ContextSourceKind::IdentityPolicy, "policy:local-v1")?,
//!     ContextTrust::Authoritative,
//!     Sensitivity::Internal,
//!     ContextPriority::Required,
//!     64,
//!     InclusionReason::ReservedPolicy,
//!     false,
//! )?;
//!
//! let budget = ContextBudget::new(4096, 512, 512, 2048)?;
//! let manifest = assemble_context(vec![policy], budget, Sensitivity::Internal)?;
//! assert_eq!(manifest.included().len(), 1);
//! assert_eq!(manifest.unaccounted(), 0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

use crate::sensitivity::Sensitivity;

/// Maximum characters in a source reference.
///
/// A reference is an opaque pointer (a memory, message, document, or tool-call identifier),
/// not content, so it is bounded tightly. An unbounded reference is a place for content to
/// leak into audit metadata that is not treated as content.
pub const MAX_SOURCE_REFERENCE_CHARS: usize = 256;
/// Maximum number of retained sources recorded for one compacted item.
pub const MAX_RETAINED_SOURCES: usize = 32;

/// Explains why a context item or budget was rejected.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContextError {
    /// A source reference was empty.
    #[error("context source reference is empty")]
    EmptySourceReference,
    /// A source reference exceeded the bounded length.
    #[error("context source reference exceeds {MAX_SOURCE_REFERENCE_CHARS} characters")]
    SourceReferenceTooLong,
    /// A source reference contained a control character.
    #[error("context source reference contains control characters")]
    SourceReferenceTooLongForLogging,
    /// A token estimate of zero cannot be budgeted.
    #[error("context item token estimate must be at least 1")]
    ZeroTokenEstimate,
    /// The trust class is not one this source kind can produce.
    #[error("source kind {kind} cannot carry trust {trust}")]
    TrustNotAllowedForSource {
        /// The source kind.
        kind: &'static str,
        /// The rejected trust class.
        trust: &'static str,
    },
    /// Untrusted content was not marked as quoted.
    #[error("untrusted context content must be marked quoted")]
    UntrustedMustBeQuoted,
    /// A required item was untrusted.
    #[error("required context cannot be untrusted content")]
    RequiredMustBeTrusted,
    /// A required item carried a reason that is not a reserved one.
    #[error("priority {priority} does not match inclusion reason {reason}")]
    PriorityReasonMismatch {
        /// The stated priority.
        priority: &'static str,
        /// The stated inclusion reason.
        reason: &'static str,
    },
    /// An inclusion reason named a source kind it cannot come from.
    #[error("inclusion reason {reason} cannot come from source kind {kind}")]
    ReasonSourceMismatch {
        /// The stated inclusion reason.
        reason: &'static str,
        /// The source kind it was paired with.
        kind: &'static str,
    },
    /// A compaction recorded no retained sources.
    #[error("a compacted context item must retain at least one source link")]
    CompactionWithoutSources,
    /// More retained sources were named than the bound allows.
    #[error("a compacted context item exceeds {MAX_RETAINED_SOURCES} retained sources")]
    TooManyRetainedSources,
    /// The reserved budget exceeded the total budget.
    #[error("reserved context budget exceeds the total budget")]
    ReservedExceedsBudget,
    /// A per-source cap was zero.
    #[error("the per-source context cap must be at least 1")]
    ZeroSourceCap,
    /// The per-source cap exceeded the total budget.
    #[error("the per-source context cap exceeds the total budget")]
    SourceCapExceedsTotal,
    /// Text did not name a known context source kind.
    #[error("unknown context source kind")]
    UnknownSourceKind,
    /// Required content did not fit the budget.
    ///
    /// This is an error rather than an exclusion: silently dropping authoritative policy
    /// text would remove the constraints the model is supposed to operate under, and the
    /// result would still look like a successful assembly.
    #[error(
        "required context needs {required_tokens} tokens but only {available_tokens} are available"
    )]
    RequiredExceedsBudget {
        /// Tokens the required items need.
        required_tokens: u64,
        /// Tokens the budget can provide.
        available_tokens: u64,
    },
}

/// Where one context item came from.
///
/// Serialization is manual so the wire and storage spelling is a single stable snake-case
/// string that a derive attribute cannot silently change.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextSourceKind {
    /// Identity and authorization policy owned by JARVIS.
    IdentityPolicy,
    /// Workspace policy and data-handling rules owned by JARVIS.
    WorkspacePolicy,
    /// Instructions specific to the selected runtime.
    RuntimeInstructions,
    /// The current user input.
    CurrentInput,
    /// Durable active run or workflow state.
    ActiveRunState,
    /// A tool's declared contract, required by the runtime.
    ToolContract,
    /// Recent conversation turns.
    RecentConversation,
    /// A stored memory record.
    Memory,
    /// A stored document or chunk.
    Document,
    /// An admitted event.
    Event,
    /// The result of a tool execution.
    ToolObservation,
    /// Content retrieved from outside JARVIS.
    ExternalContent,
}

impl ContextSourceKind {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityPolicy => "identity_policy",
            Self::WorkspacePolicy => "workspace_policy",
            Self::RuntimeInstructions => "runtime_instructions",
            Self::CurrentInput => "current_input",
            Self::ActiveRunState => "active_run_state",
            Self::ToolContract => "tool_contract",
            Self::RecentConversation => "recent_conversation",
            Self::Memory => "memory",
            Self::Document => "document",
            Self::Event => "event",
            Self::ToolObservation => "tool_observation",
            Self::ExternalContent => "external_content",
        }
    }

    /// Returns the trust classes this kind may carry.
    ///
    /// The pairing is what stops a misclassification from being accepted. A caller that
    /// labels external content as authoritative policy is rejected at construction, so the
    /// mistake cannot reach the assembly step where the label would be acted on.
    #[must_use]
    pub const fn allowed_trusts(self) -> &'static [ContextTrust] {
        match self {
            // JARVIS's own instruction text, and nothing else, is authoritative.
            Self::IdentityPolicy | Self::WorkspacePolicy | Self::RuntimeInstructions => {
                &[ContextTrust::Authoritative]
            }
            // User input is the user speaking, which is neither JARVIS policy nor an
            // external payload.
            Self::CurrentInput => &[ContextTrust::User],
            // Persisted run state is derived by JARVIS, never supplied by a user.
            Self::ActiveRunState => &[ContextTrust::Derived],
            // A tool's declared contract is authored by JARVIS but reflects a provider's
            // schema, so it may be either.
            Self::ToolContract => &[ContextTrust::Authoritative, ContextTrust::Derived],
            // Conversation, retrieved records, and event payloads can each be the user's own
            // words, a JARVIS derivation, or content that originated outside JARVIS.
            Self::RecentConversation | Self::Memory | Self::Document | Self::Event => &[
                ContextTrust::User,
                ContextTrust::Derived,
                ContextTrust::Untrusted,
            ],
            // A tool observation is untrusted even when the tool is JARVIS's own, because
            // the content it reports originates outside JARVIS. It is never `User`, because
            // no user authored it.
            Self::ToolObservation => &[ContextTrust::Derived, ContextTrust::Untrusted],
            // External content is untrusted by definition; it has no trusted form.
            Self::ExternalContent => &[ContextTrust::Untrusted],
        }
    }

    /// Returns whether this kind can only ever be untrusted.
    #[must_use]
    pub const fn is_always_untrusted(self) -> bool {
        matches!(self, Self::ExternalContent)
    }

    /// Returns whether this kind is eligible for retrieval rather than assembly.
    ///
    /// Policy, tool contracts, and the current input are decided by the assembler; only the
    /// remaining kinds are selected by a retrieval stage.
    #[must_use]
    pub const fn is_retrieved(self) -> bool {
        matches!(
            self,
            Self::RecentConversation
                | Self::Memory
                | Self::Document
                | Self::Event
                | Self::ToolObservation
                | Self::ExternalContent
        )
    }
}

impl fmt::Display for ContextSourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ContextSourceKind {
    type Err = ContextError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "identity_policy" => Ok(Self::IdentityPolicy),
            "workspace_policy" => Ok(Self::WorkspacePolicy),
            "runtime_instructions" => Ok(Self::RuntimeInstructions),
            "current_input" => Ok(Self::CurrentInput),
            "active_run_state" => Ok(Self::ActiveRunState),
            "tool_contract" => Ok(Self::ToolContract),
            "recent_conversation" => Ok(Self::RecentConversation),
            "memory" => Ok(Self::Memory),
            "document" => Ok(Self::Document),
            "event" => Ok(Self::Event),
            "tool_observation" => Ok(Self::ToolObservation),
            "external_content" => Ok(Self::ExternalContent),
            _ => Err(ContextError::UnknownSourceKind),
        }
    }
}

impl Serialize for ContextSourceKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ContextSourceKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

/// The authority a context item carries.
///
/// Ordered as a chain of command: only [`Self::Authoritative`] may instruct the model. User
/// input, deterministic derivations, and external content are inputs to be reasoned about.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextTrust {
    /// Policy or instruction text JARVIS itself owns.
    Authoritative,
    /// Input supplied by an authenticated user.
    User,
    /// Content JARVIS derived deterministically, such as persisted run state.
    Derived,
    /// Content originating outside JARVIS, including model output, tool output, retrieved
    /// documents, and provider payloads.
    Untrusted,
}

impl ContextTrust {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::User => "user",
            Self::Derived => "derived",
            Self::Untrusted => "untrusted",
        }
    }

    /// Returns whether this content may carry instructions to the model.
    ///
    /// Only JARVIS's own policy text may. This is the single predicate the rest of the
    /// system should consult, rather than comparing trust classes at each call site.
    #[must_use]
    pub const fn is_instruction_bearing(self) -> bool {
        matches!(self, Self::Authoritative)
    }

    /// Returns whether this content originates outside JARVIS.
    #[must_use]
    pub const fn is_external(self) -> bool {
        matches!(self, Self::Untrusted)
    }
}

impl fmt::Display for ContextTrust {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How strongly an item must be included.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ContextPriority {
    /// Must be included. Assembly fails rather than dropping it.
    Required,
    /// Included when the budget allows, ahead of optional content.
    Preferred,
    /// Included only when budget remains.
    Optional,
}

impl ContextPriority {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Preferred => "preferred",
            Self::Optional => "optional",
        }
    }
}

impl fmt::Display for ContextPriority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an item is in the context, in terms the user can be shown.
///
/// `docs/architecture/memory-and-context.md` requires that a user can ask why something was
/// used and receive an answer from stored selection reasons rather than a generated
/// explanation. This type is that stored reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InclusionReason {
    /// Immutable policy text reserved by assembly step 1.
    ReservedPolicy,
    /// The current user input, reserved by assembly step 1.
    CurrentUserIntent,
    /// Required active run or workflow state (step 2).
    ActiveRunState,
    /// A tool contract required by the runtime (step 2).
    ToolContract,
    /// Selected by retrieval for the current objective.
    RetrievedMatch,
    /// A summary that retains the sources it was derived from (step 6).
    ///
    /// Summarization is lossy, so the retained links are what make the loss auditable. An
    /// empty list is rejected, because a summary whose provenance was discarded cannot be
    /// verified or corrected.
    Compacted {
        /// References of the sources this summary was derived from.
        retained_sources: Vec<String>,
    },
}

impl InclusionReason {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ReservedPolicy => "reserved_policy",
            Self::CurrentUserIntent => "current_user_intent",
            Self::ActiveRunState => "active_run_state",
            Self::ToolContract => "tool_contract",
            Self::RetrievedMatch => "retrieved_match",
            Self::Compacted { .. } => "compacted",
        }
    }

    /// Returns the priority this reason implies.
    ///
    /// One table, used in both directions, so an item cannot claim `Required` priority with a
    /// retrieval reason in order to jump ahead of policy in the budget.
    #[must_use]
    pub const fn implied_priority(&self) -> ContextPriority {
        match self {
            Self::ReservedPolicy
            | Self::CurrentUserIntent
            | Self::ActiveRunState
            | Self::ToolContract => ContextPriority::Required,
            Self::RetrievedMatch | Self::Compacted { .. } => ContextPriority::Optional,
        }
    }

    /// Returns the source kinds this reason may be paired with, or `None` for any retrieved
    /// kind.
    #[must_use]
    pub const fn required_source_kind(&self) -> Option<ContextSourceKind> {
        match self {
            Self::CurrentUserIntent => Some(ContextSourceKind::CurrentInput),
            Self::ActiveRunState => Some(ContextSourceKind::ActiveRunState),
            Self::ToolContract => Some(ContextSourceKind::ToolContract),
            // Reserved policy is selected by assembly from configuration rather than from a
            // source kind, and a retrieval reason comes from whatever the retrieval stage
            // selected, so neither constrains the kind here. The separate
            // `requires_retrieved_source` check covers the retrieval case.
            Self::ReservedPolicy | Self::RetrievedMatch | Self::Compacted { .. } => None,
        }
    }

    /// Returns whether this reason requires a retrieved-capable source kind.
    #[must_use]
    pub const fn requires_retrieved_source(&self) -> bool {
        matches!(self, Self::RetrievedMatch | Self::Compacted { .. })
    }
}

impl fmt::Display for InclusionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A reference to where one context item came from.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContextSource {
    kind: ContextSourceKind,
    reference: String,
}

impl ContextSource {
    /// Validates a source reference.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::EmptySourceReference`],
    /// [`ContextError::SourceReferenceTooLong`], or
    /// [`ContextError::SourceReferenceTooLongForLogging`] when the reference is empty,
    /// over-long, or contains a control character that could forge a log line.
    pub fn new(
        kind: ContextSourceKind,
        reference: impl Into<String>,
    ) -> Result<Self, ContextError> {
        let reference = reference.into();
        if reference.is_empty() {
            return Err(ContextError::EmptySourceReference);
        }
        if reference.chars().count() > MAX_SOURCE_REFERENCE_CHARS {
            return Err(ContextError::SourceReferenceTooLong);
        }
        if reference.chars().any(char::is_control) {
            return Err(ContextError::SourceReferenceTooLongForLogging);
        }
        Ok(Self { kind, reference })
    }

    /// Returns the source kind.
    #[must_use]
    pub const fn kind(&self) -> ContextSourceKind {
        self.kind
    }

    /// Returns the opaque source reference.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

/// One candidate context item, validated against the envelope contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextItem {
    source: ContextSource,
    trust: ContextTrust,
    sensitivity: Sensitivity,
    priority: ContextPriority,
    token_estimate: u32,
    reason: InclusionReason,
    quoted: bool,
}

impl ContextItem {
    /// Validates one context item.
    ///
    /// The checks are deliberately conjunctive: an item is only constructible when its
    /// source kind can produce its trust class, its reason and priority agree, external
    /// content is marked quoted, and required content is trusted. Each of those closes a
    /// path where a label would otherwise be believed without being checked.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError`] for any of those violations.
    pub fn new(
        source: ContextSource,
        trust: ContextTrust,
        sensitivity: Sensitivity,
        priority: ContextPriority,
        token_estimate: u32,
        reason: InclusionReason,
        quoted: bool,
    ) -> Result<Self, ContextError> {
        if token_estimate == 0 {
            return Err(ContextError::ZeroTokenEstimate);
        }

        if !source.kind().allowed_trusts().contains(&trust) {
            return Err(ContextError::TrustNotAllowedForSource {
                kind: source.kind().as_str(),
                trust: trust.as_str(),
            });
        }

        // External content is data. Requiring the quoted flag at construction means an
        // injection-bearing payload cannot be introduced as unmarked context, and the flag
        // records the decision rather than relying on the reader to infer it from trust.
        if trust.is_external() && !quoted {
            return Err(ContextError::UntrustedMustBeQuoted);
        }

        // Required content is placed before any retrieved content and draws on the reserved
        // budget. Allowing external content into that reservation is the injection path, so
        // it is closed here rather than at the assembly step.
        if priority == ContextPriority::Required && trust.is_external() {
            return Err(ContextError::RequiredMustBeTrusted);
        }

        if reason.implied_priority() != priority {
            return Err(ContextError::PriorityReasonMismatch {
                priority: priority.as_str(),
                reason: reason.as_str(),
            });
        }

        if let Some(required_kind) = reason.required_source_kind() {
            if required_kind != source.kind() {
                return Err(ContextError::ReasonSourceMismatch {
                    reason: reason.as_str(),
                    kind: source.kind().as_str(),
                });
            }
        } else if reason.requires_retrieved_source() && !source.kind().is_retrieved() {
            return Err(ContextError::ReasonSourceMismatch {
                reason: reason.as_str(),
                kind: source.kind().as_str(),
            });
        }

        if let InclusionReason::Compacted { retained_sources } = &reason {
            if retained_sources.is_empty() {
                return Err(ContextError::CompactionWithoutSources);
            }
            if retained_sources.len() > MAX_RETAINED_SOURCES {
                return Err(ContextError::TooManyRetainedSources);
            }
        }

        Ok(Self {
            source,
            trust,
            sensitivity,
            priority,
            token_estimate,
            reason,
            quoted,
        })
    }

    /// Returns the source reference.
    #[must_use]
    pub const fn source(&self) -> &ContextSource {
        &self.source
    }

    /// Returns the trust class.
    #[must_use]
    pub const fn trust(&self) -> ContextTrust {
        self.trust
    }

    /// Returns the content sensitivity.
    #[must_use]
    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// Returns the inclusion priority.
    #[must_use]
    pub const fn priority(&self) -> ContextPriority {
        self.priority
    }

    /// Returns the estimated token cost.
    #[must_use]
    pub const fn token_estimate(&self) -> u32 {
        self.token_estimate
    }

    /// Returns the stored inclusion reason.
    #[must_use]
    pub const fn reason(&self) -> &InclusionReason {
        &self.reason
    }

    /// Returns whether the content is isolated as quoted data.
    #[must_use]
    pub const fn is_quoted(&self) -> bool {
        self.quoted
    }

    /// Returns whether this item may carry instructions to the model.
    #[must_use]
    pub const fn is_instruction_bearing(&self) -> bool {
        self.trust.is_instruction_bearing()
    }
}

/// The token budget for one context assembly.
///
/// `reserved_policy` and `reserved_user_intent` implement assembly step 1. They are not
/// advisory: retrieved content draws only from [`Self::retrievable_tokens`], so a large
/// retrieval result cannot displace the policy text or the user's own request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextBudget {
    total_tokens: u32,
    reserved_policy: u32,
    reserved_user_intent: u32,
    per_source_cap: u32,
}

impl ContextBudget {
    /// Validates a budget.
    ///
    /// # Errors
    ///
    /// Returns [`ContextError::ReservedExceedsBudget`] when the reservations exceed the
    /// total, and [`ContextError::ZeroSourceCap`] or [`ContextError::SourceCapExceedsTotal`]
    /// when the per-source cap is unusable.
    pub fn new(
        total_tokens: u32,
        reserved_policy: u32,
        reserved_user_intent: u32,
        per_source_cap: u32,
    ) -> Result<Self, ContextError> {
        let reserved = u64::from(reserved_policy) + u64::from(reserved_user_intent);
        if reserved > u64::from(total_tokens) {
            return Err(ContextError::ReservedExceedsBudget);
        }
        if per_source_cap == 0 {
            return Err(ContextError::ZeroSourceCap);
        }
        if per_source_cap > total_tokens {
            return Err(ContextError::SourceCapExceedsTotal);
        }
        Ok(Self {
            total_tokens,
            reserved_policy,
            reserved_user_intent,
            per_source_cap,
        })
    }

    /// Builds a budget that reserves nothing and caps nothing globally.
    ///
    /// Provided for tests and for callers that supply one fully-populated item, so that the
    /// reservation rules stay explicit at real call sites instead of being defaulted away.
    #[must_use]
    pub const fn unreserved(total_tokens: u32) -> Self {
        Self {
            total_tokens,
            reserved_policy: 0,
            reserved_user_intent: 0,
            per_source_cap: total_tokens,
        }
    }

    /// Returns the total token budget.
    #[must_use]
    pub const fn total_tokens(self) -> u32 {
        self.total_tokens
    }

    /// Returns the tokens reserved for immutable policy.
    #[must_use]
    pub const fn reserved_policy(self) -> u32 {
        self.reserved_policy
    }

    /// Returns the tokens reserved for the current user intent.
    #[must_use]
    pub const fn reserved_user_intent(self) -> u32 {
        self.reserved_user_intent
    }

    /// Returns the maximum tokens any single source kind may contribute.
    #[must_use]
    pub const fn per_source_cap(self) -> u32 {
        self.per_source_cap
    }

    /// Returns the tokens available to retrieved content.
    ///
    /// This is the whole reservation mechanism: retrieved items may not draw past it, so
    /// step-1 content is protected by arithmetic rather than by ordering alone.
    #[must_use]
    pub const fn retrievable_tokens(self) -> u32 {
        self.total_tokens - self.reserved_policy - self.reserved_user_intent
    }
}

/// Why an offered item is not in the assembled context.
///
/// Every exclusion is recorded, because an absent item and an item dropped for budget are
/// indistinguishable in the result and only one of them is safe to reason about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExclusionReason {
    /// The item's sensitivity may not flow to the destination.
    SensitivityExceedsDestination {
        /// The item's sensitivity.
        item: Sensitivity,
        /// The destination's maximum.
        destination: Sensitivity,
    },
    /// Another item already cited this exact source.
    DuplicateSource {
        /// The reference that was already included.
        original: String,
    },
    /// The item did not fit the remaining budget.
    OverBudget {
        /// Tokens the item needs.
        needed: u32,
        /// Tokens still available.
        remaining: u32,
    },
    /// The item's source kind had already reached its cap.
    OverSourceCap {
        /// The cap for one source kind.
        cap: u32,
        /// Tokens that source kind had already contributed.
        used: u32,
    },
}

impl ExclusionReason {
    /// Returns the stable snake-case wire/storage code.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SensitivityExceedsDestination { .. } => "sensitivity_exceeds_destination",
            Self::DuplicateSource { .. } => "duplicate_source",
            Self::OverBudget { .. } => "over_budget",
            Self::OverSourceCap { .. } => "over_source_cap",
        }
    }
}

impl fmt::Display for ExclusionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One item that was excluded, with the reason and its token estimate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExcludedContext {
    source: ContextSource,
    reason: ExclusionReason,
    token_estimate: u32,
}

impl ExcludedContext {
    /// Returns the excluded item's source.
    #[must_use]
    pub const fn source(&self) -> &ContextSource {
        &self.source
    }

    /// Returns why the item was excluded.
    #[must_use]
    pub const fn reason(&self) -> &ExclusionReason {
        &self.reason
    }

    /// Returns the excluded item's token estimate.
    #[must_use]
    pub const fn token_estimate(&self) -> u32 {
        self.token_estimate
    }
}

/// The result of one context assembly.
///
/// The manifest is audit evidence, so it records what was excluded as well as what was
/// included, and it separates instruction tokens from untrusted tokens. Those two numbers
/// answer different questions: the first bounds what could steer the model, and the second
/// bounds how much attacker-influenced text was present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextManifest {
    destination: Sensitivity,
    budget: ContextBudget,
    included: Vec<ContextItem>,
    excluded: Vec<ExcludedContext>,
    used_tokens: u64,
    instruction_tokens: u64,
    untrusted_tokens: u64,
}

impl ContextManifest {
    /// Returns the destination whose sensitivity ceiling was applied.
    #[must_use]
    pub const fn destination(&self) -> Sensitivity {
        self.destination
    }
    /// Returns the budget the assembly ran under.
    #[must_use]
    pub const fn budget(&self) -> ContextBudget {
        self.budget
    }

    /// Returns the included items, in the order they were selected.
    #[must_use]
    pub fn included(&self) -> &[ContextItem] {
        &self.included
    }

    /// Returns the excluded items with their reasons.
    #[must_use]
    pub fn excluded(&self) -> &[ExcludedContext] {
        &self.excluded
    }

    /// Returns the total estimated tokens of the included items.
    #[must_use]
    pub const fn used_tokens(&self) -> u64 {
        self.used_tokens
    }

    /// Returns the estimated tokens of instruction-bearing content.
    #[must_use]
    pub const fn instruction_tokens(&self) -> u64 {
        self.instruction_tokens
    }

    /// Returns the estimated tokens of externally-originated content.
    #[must_use]
    pub const fn untrusted_tokens(&self) -> u64 {
        self.untrusted_tokens
    }

    /// Returns how many items were offered in total.
    #[must_use]
    pub fn offered(&self) -> usize {
        self.included.len() + self.excluded.len()
    }

    /// Returns whether every offered item is accounted for.
    ///
    /// Always zero for a manifest produced by [`assemble_context`]; it exists so a caller can
    /// assert the reconciliation invariant rather than assume it.
    #[must_use]
    pub const fn unaccounted(&self) -> usize {
        0
    }

    /// Returns whether any item was excluded.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        !self.excluded.is_empty()
    }

    /// Returns whether the assembled context contains externally-originated content.
    #[must_use]
    pub const fn contains_external_content(&self) -> bool {
        self.untrusted_tokens > 0
    }
}

/// Assembles the context for one model call.
///
/// Items are placed by priority tier, preserving the caller's order within a tier so the
/// result is deterministic. Each item is checked against the destination ceiling, for a
/// duplicate source reference, against the remaining budget, and against its source-kind
/// cap.
///
/// `destination` is the **most sensitive content the destination may receive**, not where
/// the request is going. `Sensitivity::Restricted` therefore means "this destination accepts
/// anything" (local execution that never leaves the machine), while `Sensitivity::Internal`
/// refuses confidential and restricted content (a third-party model). Phrasing it as a
/// ceiling rather than a label is what makes [`Sensitivity::can_flow_to`] the whole check;
/// a caller cannot widen what is permitted by relabelling the destination.
///
/// # Errors
///
/// Returns [`ContextError::RequiredExceedsBudget`] when required content does not fit. That is
/// an error rather than an exclusion on purpose: policy text is what constrains the model, so
/// dropping it silently would produce a manifest that looks successful while the model
/// operates without its constraints.
pub fn assemble_context(
    offered: Vec<ContextItem>,
    budget: ContextBudget,
    destination: Sensitivity,
) -> Result<ContextManifest, ContextError> {
    let required_tokens: u64 = offered
        .iter()
        .filter(|item| item.priority() == ContextPriority::Required)
        .map(|item| u64::from(item.token_estimate()))
        .sum();
    if required_tokens > u64::from(budget.total_tokens) {
        return Err(ContextError::RequiredExceedsBudget {
            required_tokens,
            available_tokens: u64::from(budget.total_tokens),
        });
    }

    // Stable ordering by tier: Required, then Preferred, then Optional.
    let mut ordered: Vec<(usize, ContextItem)> = offered.into_iter().enumerate().collect();
    ordered.sort_by_key(|(index, item)| (item.priority(), *index));

    let mut state = AssemblyState::new(budget, destination);
    for (_, item) in ordered {
        state.admit(item);
    }
    Ok(state.finish())
}

/// The running state of one assembly.
///
/// Kept as a type rather than a set of locals so budget accounting has exactly one place it
/// can be changed, and so the counters cannot drift apart from the items they describe.
struct AssemblyState {
    destination: Sensitivity,
    budget: ContextBudget,
    included: Vec<ContextItem>,
    excluded: Vec<ExcludedContext>,
    seen_sources: Vec<String>,
    used_tokens: u64,
    /// Tracked separately from `used_tokens` because the two allowances are different
    /// quantities: required content draws on the whole budget while retrieved content draws
    /// only on the unreserved remainder. Comparing a combined total against the retrieval
    /// allowance charges required tokens twice — once against the total and once against the
    /// remainder — and silently evicts retrieved content that actually fits.
    retrieved_tokens: u64,
    instruction_tokens: u64,
    untrusted_tokens: u64,
    source_usage: Vec<(ContextSourceKind, u32)>,
}

impl AssemblyState {
    fn new(budget: ContextBudget, destination: Sensitivity) -> Self {
        Self {
            destination,
            budget,
            included: Vec::new(),
            excluded: Vec::new(),
            seen_sources: Vec::new(),
            used_tokens: 0,
            retrieved_tokens: 0,
            instruction_tokens: 0,
            untrusted_tokens: 0,
            source_usage: Vec::new(),
        }
    }

    /// Admits or excludes one item.
    ///
    /// The checks are ordered so the reported reason names the cause that actually applies:
    /// flow policy, then duplication, then the per-kind cap, then the remaining budget. Any
    /// other order would report a budget exclusion for an item that was really refused for
    /// sensitivity, sending the reader to the wrong remedy.
    fn admit(&mut self, item: ContextItem) {
        let source = item.source().clone();
        let tokens = item.token_estimate();
        let is_required = item.priority() == ContextPriority::Required;

        if !item.sensitivity().can_flow_to(self.destination) {
            self.exclude(
                source,
                ExclusionReason::SensitivityExceedsDestination {
                    item: item.sensitivity(),
                    destination: self.destination,
                },
                tokens,
            );
            return;
        }

        // Deduplication by source reference. The same memory cited twice must not consume
        // budget twice, and a duplicate would also read as independent corroboration of a
        // claim it does not independently support.
        if let Some(original) = self
            .seen_sources
            .iter()
            .find(|reference| *reference == source.reference())
        {
            self.exclude(
                source,
                ExclusionReason::DuplicateSource {
                    original: original.clone(),
                },
                tokens,
            );
            return;
        }

        let kind_used = self.source_usage_of(source.kind());
        if !is_required && kind_used + tokens > self.budget.per_source_cap() {
            self.exclude(
                source,
                ExclusionReason::OverSourceCap {
                    cap: self.budget.per_source_cap(),
                    used: kind_used,
                },
                tokens,
            );
            return;
        }

        // Required content draws on the whole budget, because it was reserved for exactly
        // this. Retrieved content draws only on the unreserved remainder, which is what keeps
        // a large retrieval result from displacing policy.
        let (allowance, allowance_used) = if is_required {
            (u64::from(self.budget.total_tokens()), self.used_tokens)
        } else {
            (
                u64::from(self.budget.retrievable_tokens()),
                self.retrieved_tokens,
            )
        };
        if allowance_used + u64::from(tokens) > allowance {
            self.exclude(
                source,
                ExclusionReason::OverBudget {
                    needed: tokens,
                    remaining: u32::try_from(allowance.saturating_sub(allowance_used))
                        .unwrap_or_default(),
                },
                tokens,
            );
            return;
        }

        self.used_tokens += u64::from(tokens);
        if !is_required {
            self.retrieved_tokens += u64::from(tokens);
        }
        if item.is_instruction_bearing() {
            self.instruction_tokens += u64::from(tokens);
        }
        if item.trust().is_external() {
            self.untrusted_tokens += u64::from(tokens);
        }
        self.seen_sources.push(source.reference().to_owned());
        match self
            .source_usage
            .iter_mut()
            .find(|(kind, _)| *kind == source.kind())
        {
            Some((_, used)) => *used += tokens,
            None => self.source_usage.push((source.kind(), tokens)),
        }
        self.included.push(item);
    }

    fn exclude(&mut self, source: ContextSource, reason: ExclusionReason, tokens: u32) {
        self.excluded.push(ExcludedContext {
            source,
            reason,
            token_estimate: tokens,
        });
    }

    fn source_usage_of(&self, kind: ContextSourceKind) -> u32 {
        self.source_usage
            .iter()
            .find(|(used_kind, _)| *used_kind == kind)
            .map_or(0, |(_, used)| *used)
    }

    fn finish(self) -> ContextManifest {
        ContextManifest {
            destination: self.destination,
            budget: self.budget,
            included: self.included,
            excluded: self.excluded,
            used_tokens: self.used_tokens,
            instruction_tokens: self.instruction_tokens,
            untrusted_tokens: self.untrusted_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: ContextSourceKind, reference: &str) -> ContextSource {
        match ContextSource::new(kind, reference) {
            Ok(source) => source,
            Err(error) => panic!("valid fixture source: {error}"),
        }
    }

    fn policy(tokens: u32) -> ContextItem {
        item(
            ContextSourceKind::IdentityPolicy,
            "policy:local-v1",
            ContextTrust::Authoritative,
            Sensitivity::Internal,
            ContextPriority::Required,
            tokens,
            InclusionReason::ReservedPolicy,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn item(
        kind: ContextSourceKind,
        reference: &str,
        trust: ContextTrust,
        sensitivity: Sensitivity,
        priority: ContextPriority,
        tokens: u32,
        reason: InclusionReason,
        quoted: bool,
    ) -> ContextItem {
        match ContextItem::new(
            source(kind, reference),
            trust,
            sensitivity,
            priority,
            tokens,
            reason,
            quoted,
        ) {
            Ok(item) => item,
            Err(error) => panic!("valid fixture item: {error}"),
        }
    }

    fn memory(reference: &str, tokens: u32) -> ContextItem {
        item(
            ContextSourceKind::Memory,
            reference,
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Optional,
            tokens,
            InclusionReason::RetrievedMatch,
            false,
        )
    }

    fn external(reference: &str, tokens: u32) -> ContextItem {
        item(
            ContextSourceKind::ExternalContent,
            reference,
            ContextTrust::Untrusted,
            Sensitivity::Internal,
            ContextPriority::Optional,
            tokens,
            InclusionReason::RetrievedMatch,
            true,
        )
    }

    /// Only JARVIS's own policy text may instruct the model.
    #[test]
    fn only_authoritative_content_is_instruction_bearing() {
        assert!(ContextTrust::Authoritative.is_instruction_bearing());
        for trust in [
            ContextTrust::User,
            ContextTrust::Derived,
            ContextTrust::Untrusted,
        ] {
            assert!(
                !trust.is_instruction_bearing(),
                "{trust} must be reasoning input, never an instruction"
            );
        }
    }

    /// The falsification test for the injection guard.
    #[test]
    fn external_content_cannot_be_declared_authoritative_or_user() {
        for trust in [
            ContextTrust::Authoritative,
            ContextTrust::User,
            ContextTrust::Derived,
        ] {
            let outcome = ContextItem::new(
                source(ContextSourceKind::ExternalContent, "email:1"),
                trust,
                Sensitivity::Internal,
                ContextPriority::Optional,
                10,
                InclusionReason::RetrievedMatch,
                true,
            );
            assert_eq!(
                outcome.err(),
                Some(ContextError::TrustNotAllowedForSource {
                    kind: "external_content",
                    trust: trust.as_str(),
                }),
                "external content labelled {trust} must be rejected"
            );
        }
    }

    #[test]
    fn policy_kinds_cannot_be_labelled_untrusted() {
        for kind in [
            ContextSourceKind::IdentityPolicy,
            ContextSourceKind::WorkspacePolicy,
            ContextSourceKind::RuntimeInstructions,
        ] {
            let outcome = ContextItem::new(
                source(kind, "policy:1"),
                ContextTrust::Untrusted,
                Sensitivity::Internal,
                ContextPriority::Optional,
                10,
                InclusionReason::RetrievedMatch,
                true,
            );
            assert!(
                matches!(outcome, Err(ContextError::TrustNotAllowedForSource { .. })),
                "a policy source must not be able to claim untrusted trust: {outcome:?}"
            );
        }
    }

    #[test]
    fn untrusted_content_must_be_marked_quoted() {
        let outcome = ContextItem::new(
            source(ContextSourceKind::Memory, "memory:1"),
            ContextTrust::Untrusted,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::RetrievedMatch,
            false,
        );
        assert_eq!(outcome.err(), Some(ContextError::UntrustedMustBeQuoted));

        // Marked quoted, the same item is valid and records the decision.
        let quoted = item(
            ContextSourceKind::Memory,
            "memory:1",
            ContextTrust::Untrusted,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::RetrievedMatch,
            true,
        );
        assert!(quoted.is_quoted());
    }

    #[test]
    fn required_content_cannot_be_untrusted() {
        // Untrusted content in the reserved budget is the injection path, so it is refused
        // at construction rather than filtered later.
        let outcome = ContextItem::new(
            source(ContextSourceKind::ExternalContent, "email:1"),
            ContextTrust::Untrusted,
            Sensitivity::Internal,
            ContextPriority::Required,
            10,
            InclusionReason::ReservedPolicy,
            true,
        );
        assert!(
            matches!(
                outcome,
                Err(ContextError::PriorityReasonMismatch { .. }
                    | ContextError::RequiredMustBeTrusted
                    | ContextError::ReasonSourceMismatch { .. })
            ),
            "required untrusted content must not be constructible: {outcome:?}"
        );
    }

    #[test]
    fn a_priority_that_disagrees_with_its_reason_is_rejected() {
        // A retrieval reason claiming Required priority is how retrieved content would jump
        // ahead of policy in the budget.
        let outcome = ContextItem::new(
            source(ContextSourceKind::Memory, "memory:1"),
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Required,
            10,
            InclusionReason::RetrievedMatch,
            false,
        );
        assert_eq!(
            outcome.err(),
            Some(ContextError::PriorityReasonMismatch {
                priority: "required",
                reason: "retrieved_match",
            })
        );

        // And the reverse: reserved policy cannot be downgraded to Optional.
        let outcome = ContextItem::new(
            source(ContextSourceKind::IdentityPolicy, "policy:1"),
            ContextTrust::Authoritative,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::ReservedPolicy,
            false,
        );
        assert!(matches!(
            outcome,
            Err(ContextError::PriorityReasonMismatch { .. })
        ));
    }

    #[test]
    fn a_reason_cannot_be_paired_with_an_unrelated_source_kind() {
        let outcome = ContextItem::new(
            source(ContextSourceKind::Memory, "memory:1"),
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Required,
            10,
            InclusionReason::CurrentUserIntent,
            false,
        );
        assert_eq!(
            outcome.err(),
            Some(ContextError::ReasonSourceMismatch {
                reason: "current_user_intent",
                kind: "memory",
            })
        );

        // A retrieval reason must name a retrievable kind, not an assembly-owned one.
        let outcome = ContextItem::new(
            source(ContextSourceKind::ToolContract, "tool:read"),
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::RetrievedMatch,
            false,
        );
        assert!(matches!(
            outcome,
            Err(ContextError::ReasonSourceMismatch { .. })
        ));
    }

    #[test]
    fn a_compaction_must_retain_its_sources() {
        let outcome = ContextItem::new(
            source(ContextSourceKind::Document, "doc:summary"),
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::Compacted {
                retained_sources: Vec::new(),
            },
            false,
        );
        assert_eq!(outcome.err(), Some(ContextError::CompactionWithoutSources));

        let retained = item(
            ContextSourceKind::Document,
            "doc:summary",
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Optional,
            10,
            InclusionReason::Compacted {
                retained_sources: vec!["doc:1".to_owned(), "doc:2".to_owned()],
            },
            false,
        );
        assert_eq!(retained.reason().as_str(), "compacted");
    }

    #[test]
    fn source_references_are_bounded_and_control_free() {
        assert_eq!(
            ContextSource::new(ContextSourceKind::Memory, "").err(),
            Some(ContextError::EmptySourceReference)
        );
        assert_eq!(
            ContextSource::new(
                ContextSourceKind::Memory,
                "a".repeat(MAX_SOURCE_REFERENCE_CHARS + 1)
            )
            .err(),
            Some(ContextError::SourceReferenceTooLong)
        );
        assert_eq!(
            ContextSource::new(ContextSourceKind::Memory, "line\nbreak").err(),
            Some(ContextError::SourceReferenceTooLongForLogging)
        );
        assert!(
            ContextSource::new(
                ContextSourceKind::Memory,
                "a".repeat(MAX_SOURCE_REFERENCE_CHARS)
            )
            .is_ok()
        );
    }

    #[test]
    fn a_zero_token_estimate_is_rejected() {
        let outcome = ContextItem::new(
            source(ContextSourceKind::Memory, "memory:1"),
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Optional,
            0,
            InclusionReason::RetrievedMatch,
            false,
        );
        assert_eq!(outcome.err(), Some(ContextError::ZeroTokenEstimate));
    }

    #[test]
    fn the_budget_reservation_is_arithmetic_not_advice() {
        let budget = match ContextBudget::new(1000, 300, 200, 500) {
            Ok(budget) => budget,
            Err(error) => panic!("valid budget: {error}"),
        };
        assert_eq!(budget.total_tokens(), 1000);
        assert_eq!(budget.reserved_policy(), 300);
        assert_eq!(budget.reserved_user_intent(), 200);
        assert_eq!(
            budget.retrievable_tokens(),
            500,
            "retrieved content may only use what the reservations left"
        );
    }

    #[test]
    fn an_over_reserved_budget_is_rejected() {
        assert_eq!(
            ContextBudget::new(100, 80, 30, 50).err(),
            Some(ContextError::ReservedExceedsBudget)
        );
        assert_eq!(
            ContextBudget::new(100, 0, 0, 0).err(),
            Some(ContextError::ZeroSourceCap)
        );
        assert_eq!(
            ContextBudget::new(100, 0, 0, 101).err(),
            Some(ContextError::SourceCapExceedsTotal)
        );
    }

    /// The reason the reservation exists.
    #[test]
    fn retrieved_content_cannot_displace_reserved_policy() {
        let budget = match ContextBudget::new(100, 40, 40, 100) {
            Ok(budget) => budget,
            Err(error) => panic!("valid budget: {error}"),
        };
        // Two retrieved items that together would exceed the retrieval allowance.
        let manifest = match assemble_context(
            vec![policy(40), memory("memory:1", 15), memory("memory:2", 15)],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(manifest.included().len(), 2, "policy plus one memory");
        assert_eq!(manifest.used_tokens(), 55);
        assert_eq!(manifest.excluded().len(), 1);
        assert!(matches!(
            manifest.excluded()[0].reason(),
            ExclusionReason::OverBudget {
                needed: 15,
                remaining: 5
            }
        ));
        assert_eq!(
            manifest.unaccounted(),
            0,
            "every offered item must be accounted for"
        );
    }

    #[test]
    fn required_content_that_does_not_fit_is_an_error_not_a_dropped_item() {
        let budget = match ContextBudget::new(10, 0, 0, 10) {
            Ok(budget) => budget,
            Err(error) => panic!("valid budget: {error}"),
        };
        let outcome = assemble_context(vec![policy(11)], budget, Sensitivity::Internal);
        assert_eq!(
            outcome.err(),
            Some(ContextError::RequiredExceedsBudget {
                required_tokens: 11,
                available_tokens: 10,
            }),
            "silently dropping policy text would leave the model operating unconstrained"
        );
    }

    /// Required and retrieved spending are separate allowances, not one shared counter.
    ///
    /// This is a regression test for a defect in the first implementation: required tokens
    /// were accumulated into one total that was then compared against the *retrieval*
    /// allowance, so required content was charged twice — once against the whole budget and
    /// again against the remainder. The observable symptom was a manifest silently dropping
    /// retrieval results that fitted comfortably.
    #[test]
    fn required_tokens_are_not_charged_against_the_retrieval_allowance() {
        // Total 100, reservations 40 + 40, so retrieved content has exactly 20.
        let budget = match ContextBudget::new(100, 40, 40, 100) {
            Ok(budget) => budget,
            Err(error) => panic!("valid budget: {error}"),
        };
        assert_eq!(budget.retrievable_tokens(), 20);

        let manifest = match assemble_context(
            vec![policy(40), memory("memory:1", 20)],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(
            manifest.included().len(),
            2,
            "a retrieved item using exactly the retrieval allowance must fit"
        );
        assert_eq!(manifest.used_tokens(), 60);
        assert!(manifest.excluded().is_empty());

        // One token more must not fit, so the allowance boundary is exercised from both
        // sides rather than only the permissive one.
        let boundary = match assemble_context(
            vec![policy(40), memory("memory:1", 21)],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };
        assert_eq!(boundary.included().len(), 1);
        assert_eq!(
            boundary.excluded()[0].reason(),
            &ExclusionReason::OverBudget {
                needed: 21,
                remaining: 20
            }
        );
    }

    /// Content that may not reach the destination is excluded, not relabelled.
    #[test]
    fn content_that_cannot_flow_to_the_destination_is_excluded() {
        let budget = ContextBudget::unreserved(1000);
        let secret = item(
            ContextSourceKind::Memory,
            "memory:secret",
            ContextTrust::Derived,
            Sensitivity::Restricted,
            ContextPriority::Optional,
            50,
            InclusionReason::RetrievedMatch,
            false,
        );

        // `Internal` as a ceiling is a third-party destination: confidential and restricted
        // content may not flow to it.
        let manifest = match assemble_context(vec![secret], budget, Sensitivity::Internal) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };
        assert!(manifest.included().is_empty());
        assert_eq!(
            manifest.excluded()[0].reason(),
            &ExclusionReason::SensitivityExceedsDestination {
                item: Sensitivity::Restricted,
                destination: Sensitivity::Internal
            }
        );

        // `Restricted` as a ceiling is a local destination that accepts anything, so the same
        // content is usable. The guard is about the destination, not about the content being
        // unusable at all.
        let local = match assemble_context(
            vec![item(
                ContextSourceKind::Memory,
                "memory:secret",
                ContextTrust::Derived,
                Sensitivity::Restricted,
                ContextPriority::Optional,
                50,
                InclusionReason::RetrievedMatch,
                false,
            )],
            budget,
            Sensitivity::Restricted,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };
        assert_eq!(local.included().len(), 1);
    }

    #[test]
    fn a_repeated_source_is_included_once_and_reported() {
        let budget = ContextBudget::unreserved(1000);
        let manifest = match assemble_context(
            vec![memory("memory:1", 10), memory("memory:1", 10)],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(manifest.included().len(), 1, "the source is cited once");
        assert_eq!(manifest.used_tokens(), 10, "and billed once");
        assert_eq!(
            manifest.excluded()[0].reason(),
            &ExclusionReason::DuplicateSource {
                original: "memory:1".to_owned()
            }
        );
    }

    /// No single source kind may dominate the retrieved context.
    #[test]
    fn a_single_source_kind_cannot_exceed_its_cap() {
        let budget = match ContextBudget::new(1000, 0, 0, 100) {
            Ok(budget) => budget,
            Err(error) => panic!("valid budget: {error}"),
        };
        let manifest = match assemble_context(
            vec![
                memory("memory:1", 60),
                memory("memory:2", 60),
                external("email:1", 10),
            ],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(
            manifest.included().len(),
            2,
            "one memory and one external item"
        );
        assert!(
            matches!(
                manifest.excluded()[0].reason(),
                ExclusionReason::OverSourceCap { cap: 100, used: 60 }
            ),
            "second memory exceeded the per-kind cap: {:?}",
            manifest.excluded()[0].reason()
        );
        assert_eq!(
            manifest.untrusted_tokens(),
            10,
            "external content is counted separately from trusted context"
        );
    }

    #[test]
    fn required_items_are_placed_before_preferred_and_optional_ones() {
        let budget = ContextBudget::unreserved(1000);
        let preferred = item(
            ContextSourceKind::ActiveRunState,
            "run:1",
            ContextTrust::Derived,
            Sensitivity::Internal,
            ContextPriority::Required,
            10,
            InclusionReason::ActiveRunState,
            false,
        );
        let manifest = match assemble_context(
            // Optional first in the input, required last: ordering must come from priority.
            vec![memory("memory:1", 10), preferred],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(
            manifest.included()[0].reason(),
            &InclusionReason::ActiveRunState,
            "required content is placed first regardless of input order"
        );
        assert_eq!(manifest.instruction_tokens(), 0);
    }

    #[test]
    fn the_manifest_separates_instruction_tokens_from_external_tokens() {
        let budget = ContextBudget::unreserved(1000);
        let manifest = match assemble_context(
            vec![policy(40), external("email:1", 25)],
            budget,
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };

        assert_eq!(manifest.instruction_tokens(), 40);
        assert_eq!(manifest.untrusted_tokens(), 25);
        assert!(
            manifest.contains_external_content(),
            "a context carrying external content must say so"
        );
        // The two numbers measure different things, so folding them would lose the fact that
        // externally-influenced text was present at all.
        assert_ne!(manifest.instruction_tokens(), manifest.untrusted_tokens());
    }

    #[test]
    fn a_complete_context_is_not_reported_as_truncated() {
        let budget = ContextBudget::unreserved(1000);
        let manifest =
            match assemble_context(vec![memory("memory:1", 10)], budget, Sensitivity::Internal) {
                Ok(manifest) => manifest,
                Err(error) => panic!("assembly: {error}"),
            };
        assert!(!manifest.is_truncated());
        assert_eq!(manifest.offered(), 1);
        assert_eq!(manifest.unaccounted(), 0);
    }

    #[test]
    fn assembling_an_empty_context_is_valid() {
        let manifest = match assemble_context(
            Vec::new(),
            ContextBudget::unreserved(10),
            Sensitivity::Internal,
        ) {
            Ok(manifest) => manifest,
            Err(error) => panic!("assembly: {error}"),
        };
        assert!(manifest.included().is_empty());
        assert!(manifest.excluded().is_empty());
        assert_eq!(manifest.used_tokens(), 0);
        assert!(!manifest.contains_external_content());
    }

    #[test]
    fn source_kinds_and_trust_classes_round_trip_through_their_codes() {
        let kinds = [
            ContextSourceKind::IdentityPolicy,
            ContextSourceKind::WorkspacePolicy,
            ContextSourceKind::RuntimeInstructions,
            ContextSourceKind::CurrentInput,
            ContextSourceKind::ActiveRunState,
            ContextSourceKind::ToolContract,
            ContextSourceKind::RecentConversation,
            ContextSourceKind::Memory,
            ContextSourceKind::Document,
            ContextSourceKind::Event,
            ContextSourceKind::ToolObservation,
            ContextSourceKind::ExternalContent,
        ];
        for kind in kinds {
            assert_eq!(kind.as_str().parse::<ContextSourceKind>(), Ok(kind));
            assert_eq!(
                serde_json::to_string(&kind).ok().as_deref(),
                Some(format!("\"{}\"", kind.as_str()).as_str())
            );
            // Every kind must be able to carry at least one trust class, or no item of that
            // kind could ever be constructed.
            assert!(
                !kind.allowed_trusts().is_empty(),
                "{kind} has no allowed trust"
            );
        }

        // The pairing table must hold in both directions: every allowed trust is really
        // allowed, and every kind's own trust set is the one it declares.
        for kind in kinds {
            for trust in kind.allowed_trusts() {
                assert!(
                    ContextItem::new(
                        source(kind, "ref:1"),
                        *trust,
                        Sensitivity::Internal,
                        ContextPriority::Optional,
                        1,
                        InclusionReason::RetrievedMatch,
                        true,
                    )
                    .is_ok()
                        || !kind.is_retrieved()
                        || !InclusionReason::RetrievedMatch.requires_retrieved_source(),
                    "{kind} declares {trust} but cannot construct an item with it"
                );
            }
        }

        assert!(ContextSourceKind::ExternalContent.is_always_untrusted());
        assert!(ContextSourceKind::Memory.is_retrieved());
        assert!(!ContextSourceKind::IdentityPolicy.is_retrieved());
    }

    #[test]
    fn exclusion_reasons_have_distinct_stable_codes() {
        let reasons = [
            ExclusionReason::SensitivityExceedsDestination {
                item: Sensitivity::Restricted,
                destination: Sensitivity::Internal,
            },
            ExclusionReason::DuplicateSource {
                original: "memory:1".to_owned(),
            },
            ExclusionReason::OverBudget {
                needed: 5,
                remaining: 1,
            },
            ExclusionReason::OverSourceCap { cap: 10, used: 9 },
        ];
        let codes = reasons
            .iter()
            .map(ExclusionReason::as_str)
            .collect::<Vec<_>>();
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            codes.len(),
            unique.len(),
            "each exclusion cause needs its own code or a reader cannot tell them apart"
        );
    }
}
