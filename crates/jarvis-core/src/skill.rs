//! Skills: a procedure that names already-granted tools, and never authority.
//!
//! `ADR-0117` fixes the trust boundary this module implements, and the whole of it is one sentence: **a
//! skill describes _how_ to do something the user has already permitted.** It is not a grant, cannot widen
//! one, and carries no authority of its own.
//!
//! # Why the vocabulary is shaped to make authority unrepresentable
//!
//! "Skill" is the name two other designs in this class give to **text that is loaded and then acted on**:
//! a procedure injected beside the tool list, with its effects gated by a pattern list or a coarse
//! approval. That step is incompatible with this platform, because `AGENTS.md` states the boundary it is
//! built on — *the model may request an effect; deterministic Rust policy decides whether it may happen* —
//! and a skill is a new route by which an instruction reaches the model.
//!
//! So there is deliberately **no field** here for a grant, a scope, a pre-approved effect, or a chosen
//! approver. A skill is a [`SkillId`], a source, a version, a step list, and prose. There is no
//! constructor that accepts authority and no field one could arrive in, which is the `P3-006c` shape: the
//! rule is enforced by what the type can express rather than by a check each reader must remember.
//!
//! # The three states, and why there are not four
//!
//! [`SkillState`] is `Proposed`, `Active`, or `Archived`. `Proposed` is not decoration: `ADR-0117` §4
//! makes promotion an approval, so an **agent-authored** skill must begin as a proposal and become usable
//! only through a durable, attributable decision. A record has no `Deleted` state because deletion is the
//! revision's content being removed and the row's tombstone status, which is storage's concern rather than
//! a state a value can be observed in — the same split [`MemoryStatus`](crate::MemoryStatus) makes.
//!
//! # A step names a tool at a version, because "which procedure ran" must be answerable
//!
//! [`SkillStep::tool`] is a [`ToolId`] and [`SkillStep::tool_version`] is the version a revision was
//! authored against. `ADR-0117` §5 requires that a stored procedure carries "the tools it names at that
//! version", and the reason is audit rather than validation: a later revision of a tool can change a
//! schema, and "which version of the tool did this procedure address" is not reconstructible from the
//! identifier alone.
//!
//! The step's version is **evidence, not a grant**. Whether the actor's current grant covers the step is
//! decided when the step runs ([`jarvis_tools`'s pipeline]), never when the skill is loaded, because a
//! grant revoked between the two must refuse the step and only an execution-time check can see that.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::approval::MAX_APPROVER_ID_CHARS;
use crate::id::{SkillId, WorkspaceId};
use crate::memory::MemorySource;
use crate::sensitivity::Sensitivity;
use crate::timestamp::UtcTimestamp;

/// The longest prose a skill may carry as its description.
///
/// Bounded because the description reaches the model: a skill's prose is retrieved content, so an
/// unbounded one is a single record that can consume an entire context budget — the same reasoning
/// [`MAX_MEMORY_CONTENT_CHARS`](crate::memory::MAX_MEMORY_CONTENT_CHARS) gives. The number is larger
/// because a procedure's prose is legitimately longer than a claim's text, and a procedure that has to be
/// split across records is not a procedure.
pub const MAX_SKILL_DESCRIPTION_CHARS: usize = 8192;

/// The longest text one step may carry.
///
/// The step's own instruction, bounded as a fraction of the description's budget: a step instruction is
/// one action's worth of prose, and a step as long as the whole skill is a sign the procedure was not
/// decomposed rather than a limit being hit.
pub const MAX_SKILL_STEP_CHARS: usize = 1024;

/// The most steps one skill may name.
///
/// A bound rather than a preference. A skill becomes a sequence of tool requests, and an unbounded list is
/// a single record that can schedule unbounded work — so the number is the point at which a procedure
/// should have become a workflow (`ADR-0117`'s own distinction) rather than grown.
pub const MAX_SKILL_STEPS: usize = 64;

/// The most external fields one revision may record as dropped.
///
/// Bounded so a hostile document cannot make the drop record itself unbounded — the record is stored, and
/// a format with ten thousand unknown keys must not become a ten-thousand-row audit entry.
pub const MAX_SKILL_DROPPED_FIELDS: usize = 64;

/// The longest name one dropped external field may carry.
///
/// Bounded for the same reason: the names are stored, and they come from a document this platform did not
/// write.
pub const MAX_SKILL_DROPPED_FIELD_NAME_CHARS: usize = 128;

/// The longest version string a revision may carry.
///
/// Matches `jarvis_tools`'s own bound on a tool version, because a step's `tool_version` must be able to
/// hold one.
pub const MAX_SKILL_VERSION_CHARS: usize = 64;

/// Explains why a skill was rejected.
///
/// One variant per rule, carrying the field name where the shape is wrong, so a caller learns which check
/// failed without the message forwarding the content it rejected — the rule every error type in this crate
/// follows, and it matters more here because a skill's content is model-authored.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidSkill {
    /// The description was empty, whitespace-only, or over [`MAX_SKILL_DESCRIPTION_CHARS`].
    #[error(
        "a skill description must be non-empty and at most {MAX_SKILL_DESCRIPTION_CHARS} characters"
    )]
    Description,
    /// The step list was empty or longer than [`MAX_SKILL_STEPS`].
    #[error("a skill must name between 1 and {MAX_SKILL_STEPS} steps")]
    Steps,
    /// A step's instruction text was empty, whitespace-only, or over [`MAX_SKILL_STEP_CHARS`].
    #[error(
        "a skill step instruction must be non-empty and at most {MAX_SKILL_STEP_CHARS} characters"
    )]
    StepText,
    /// The version was empty or over [`MAX_SKILL_VERSION_CHARS`].
    #[error("a skill version must be 1 to {MAX_SKILL_VERSION_CHARS} characters")]
    Version,
    /// A step's tool identifier was rejected by `jarvis_tools`'s own rule.
    ///
    /// Carries nothing: the identifier is the thing being validated, and a rejected identifier rendered
    /// into an error is how a malformed value reaches a log.
    #[error("a skill step names a tool identifier this platform rejects")]
    ToolIdentifier,
    /// A step's tool version was empty or over [`MAX_SKILL_VERSION_CHARS`].
    #[error("a skill step's tool version must be 1 to {MAX_SKILL_VERSION_CHARS} characters")]
    ToolVersion,
    /// The source was unusable for a skill.
    ///
    /// A skill's provenance follows [`MemorySource`]'s rules, so a source that kind and trust disagree
    /// about is refused there and reported here.
    #[error("the skill source is unusable")]
    Source,
    /// A skill the **model** authored was recorded as trustworthy.
    ///
    /// `ADR-0117` §3: a self-authored procedure is `Derived` at best, because a model-authored skill
    /// re-entering a later prompt as though it were the user's instruction is the self-feeding loop the
    /// inference boundary exists to prevent.
    #[error(
        "a model-authored skill must carry derived or untrusted provenance, never authoritative"
    )]
    ModelAuthoredTrust,
    /// A promotion named no approver.
    ///
    /// `ADR-0043` requires a decision to be attributable, so an empty approver is refused rather than stored.
    /// Its own variant, separate from self-approval, because the remedies differ: this one needs an approver
    /// *named*, while the other needs a *different* actor to decide.
    #[error("a skill promotion must name its approver")]
    PromotionUnattributed,
    /// A promotion named the revision's own author as its approver.
    ///
    /// **The boundary `ADR-0117` §4 states in words**: *"an agent that could author a procedure and promote
    /// it would have authored its own effect."* The same rule a tool approval applies to a call, where an
    /// approver equal to the requester is not an approval at all.
    #[error("a skill revision's own author cannot promote it")]
    PromotionSelfApproval,
    /// The approver identity was longer than [`MAX_APPROVER_ID_CHARS`], so it cannot be stored.
    ///
    /// The same bound the `approvals` table's `decided_by` column carries, and the same reason: a promotion is
    /// an approval, so its approver is an approval's approver and one identity must not have two lengths. A
    /// value accepted here and refused by the column would be a write that fails after the domain said the
    /// record was valid.
    #[error("a skill approver identity must be at most {MAX_APPROVER_ID_CHARS} characters")]
    ApproverTooLong,
    /// The revision's state does not accept the transition that was requested.
    ///
    /// # Why one variant rather than one per verb
    ///
    /// The fact is always the same — "this state is not the one this transition starts from" — and only the
    /// verb differs, so the verb is a **field**. Three variants would be three names for one condition, and
    /// [`Self::SelfReference`] was doing this job for `archive` and `restore` before: a variant that means "a
    /// value referring to itself" was reporting a *state* problem, so a caller matching on it would read a
    /// message about identity for a revision that is simply already archived.
    #[error("a skill revision cannot {transition} while it is {state}")]
    WrongState {
        /// The transition that was requested, by its verb.
        transition: &'static str,
        /// The state it was requested from.
        state: SkillState,
    },
    /// The dropped-field record was longer than [`MAX_SKILL_DROPPED_FIELDS`], or a name was blank or
    /// oversized.
    #[error(
        "at most {MAX_SKILL_DROPPED_FIELDS} dropped fields may be recorded, each 1 to \
             {MAX_SKILL_DROPPED_FIELD_NAME_CHARS} characters"
    )]
    DroppedFields,
    /// A step duplicated another step's position in the same revision.
    ///
    /// A duplicate position is refused rather than resolved, because "which of these two runs first" would
    /// otherwise depend on the order a map happened to iterate — the defect `jarvis_mcp`'s catalog refuses
    /// for a name collision chosen by configuration order.
    #[error("a skill step position was declared twice in one revision")]
    DuplicateStepPosition,
    /// The revision was superseded by, or derived from, itself.
    #[error("a skill revision cannot reference itself")]
    SelfReference,
}

/// What state a stored skill is in.
///
/// Ordered so a comparison answers "is this usable": only [`Self::Active`] is. The ordering is a
/// convenience for a filter, never a numeric score.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillState {
    /// Awaiting a promotion decision. An agent-authored skill starts here.
    Proposed,
    /// Usable. Reachable only through a promotion that named its approver.
    Active,
    /// Set aside: retained for audit, not offered for use. Reversible, unlike deletion.
    Archived,
}

impl SkillState {
    /// Returns the stable storage and wire name.
    ///
    /// Written out rather than derived from `Debug`, so a variant rename cannot silently change what a
    /// stored row says — the rule every vocabulary type in this crate follows.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }

    /// Returns every state, for a round-trip test or a storage `CHECK`.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Proposed, Self::Active, Self::Archived]
    }

    /// Returns whether a skill in this state may be offered for use.
    ///
    /// Only `Active`. The predicate the promotion rule exists to make observable: a proposal is a
    /// candidate, not a procedure, so retrieval must not offer it.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns whether this state is terminal.
    ///
    /// Never, and that is deliberate: `Archived` is reversible, which is what makes setting a skill aside
    /// different from deleting it. Deletion is the content being removed, which storage records as a
    /// tombstone rather than as a state a value can hold.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        false
    }
}

impl fmt::Display for SkillState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for SkillState {
    type Err = InvalidSkill;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "proposed" => Ok(Self::Proposed),
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            _ => Err(InvalidSkill::Version),
        }
    }
}

/// One external field a revision's source document carried and this platform refused to represent.
///
/// # Why a drop is recorded rather than ignored
///
/// `ADR-0117` §7 allows an external format to be adopted for its prose and its step list, and requires
/// that any field which would **grant authority, preselect an ungranted tool, or pre-approve an effect** be
/// dropped — with the drop *recorded*. The reason is the one `ADR-0022` applies to a request body and
/// `P3-006c` applied to a field with no reader: **a field that is accepted and then ignored is worse than
/// one that was never accepted**, because a reader of the stored skill cannot tell the format's intent from
/// this platform's behaviour.
///
/// So a revision carries the names of what it dropped and why. A reader can then see that a document said
/// `allowed_tools` and that this platform did not honour it, rather than assuming the document was silent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDroppedField {
    name: String,
    reason: DropReason,
}

impl SkillDroppedField {
    /// Records one dropped external field.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::DroppedFields`] for a blank or oversized name, because an unnamed drop
    /// cannot be reviewed — which defeats the reason the record exists.
    pub fn new(name: impl Into<String>, reason: DropReason) -> Result<Self, InvalidSkill> {
        let name = name.into();
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_SKILL_DROPPED_FIELD_NAME_CHARS {
            return Err(InvalidSkill::DroppedFields);
        }
        Ok(Self {
            name: trimmed.to_owned(),
            reason,
        })
    }

    /// Returns the external field's name, as the document spelled it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns why the field was dropped.
    #[must_use]
    pub const fn reason(&self) -> DropReason {
        self.reason
    }
}

/// Why an external field was dropped.
///
/// A closed set rather than free text, because the reason is **stored** and a reader compares it: "this
/// format grants tools and we refuse that" is a different statement from "we could not parse this field",
/// and a free-form string would let the two become indistinguishable in an audit read.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The field would have granted authority or a scope.
    ///
    /// The category `ADR-0117` §7 names first: a skill cannot widen a grant, so a field that claims to is
    /// dropped rather than represented anywhere.
    AuthorityGrant,
    /// The field would have selected a tool the actor was not granted.
    ToolSelection,
    /// The field would have pre-approved an effect.
    ///
    /// An approval binds to one intent's digest (`ADR-0018`), so a pre-approval is not an approval — it is
    /// a claim to a decision nobody made.
    PreApproval,
    /// The field would have chosen an approver.
    ///
    /// `ADR-0043` requires a decision to name its approver at decision time, so a document naming one
    /// would be preselecting the party the platform requires to be an independent identity.
    ApproverSelection,
    /// The field named something this build does not represent, and it carried no authority.
    ///
    /// This is the ordinary case for an interoperated document: a field for a rating, an icon, a vendor
    /// category. It is still recorded, so a reader can tell "we understood and kept this" from "we did not
    /// understand this" — a silence that would otherwise look identical.
    Unrepresented,
}

impl DropReason {
    /// Returns the stable storage and wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorityGrant => "authority_grant",
            Self::ToolSelection => "tool_selection",
            Self::PreApproval => "pre_approval",
            Self::ApproverSelection => "approver_selection",
            Self::Unrepresented => "unrepresented",
        }
    }

    /// Returns every reason, for a round-trip test or a storage `CHECK`.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::AuthorityGrant,
            Self::ToolSelection,
            Self::PreApproval,
            Self::ApproverSelection,
            Self::Unrepresented,
        ]
    }

    /// Returns whether dropping this field was **required** rather than merely unrepresented.
    ///
    /// The four authority-bearing reasons are required; `Unrepresented` is not. The distinction is what a
    /// reviewer reads to see how much of a document this platform refused, and it is stated as a method so
    /// the partition is asserted rather than assumed.
    #[must_use]
    pub const fn is_authority_bearing(self) -> bool {
        !matches!(self, Self::Unrepresented)
    }
}

impl fmt::Display for DropReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for DropReason {
    type Err = InvalidSkill;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "authority_grant" => Ok(Self::AuthorityGrant),
            "tool_selection" => Ok(Self::ToolSelection),
            "pre_approval" => Ok(Self::PreApproval),
            "approver_selection" => Ok(Self::ApproverSelection),
            "unrepresented" => Ok(Self::Unrepresented),
            _ => Err(InvalidSkill::DroppedFields),
        }
    }
}

/// One step of a procedure: a tool to call, at the version the revision was authored against, and prose.
///
/// # Why the tool identifier is a string here
///
/// A step's tool is validated by `jarvis_tools::ToolId`, which this crate cannot depend on — `jarvis-tools`
/// depends on `jarvis-core`, so the arrow only goes one way. The identifier therefore arrives as text and
/// is validated through a **caller-supplied validator**, so the rule keeps one home in the crate that owns
/// it rather than being restated here as a second pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillStep {
    position: u16,
    tool: String,
    tool_version: String,
    instruction: String,
}

impl SkillStep {
    /// Builds one step, validating the identifier through the supplied validator.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::ToolIdentifier`] when `validate_tool` refuses the identifier,
    /// [`InvalidSkill::ToolVersion`] for an unusable version, and [`InvalidSkill::StepText`] for blank or
    /// oversized instruction text.
    ///
    /// The validator is a parameter because the tool identifier rule lives in `jarvis-tools` and this
    /// crate cannot name it. Passing it in keeps the rule in one place; a regex here would be a second
    /// statement of the identifier's shape, and the two would be able to disagree about what a valid tool
    /// is.
    pub fn new(
        position: u16,
        tool: impl Into<String>,
        tool_version: impl Into<String>,
        instruction: impl Into<String>,
        validate_tool: impl FnOnce(&str) -> bool,
    ) -> Result<Self, InvalidSkill> {
        let tool = tool.into();
        if !validate_tool(&tool) {
            return Err(InvalidSkill::ToolIdentifier);
        }
        let tool_version = tool_version.into();
        if tool_version.is_empty()
            || tool_version.chars().count() > MAX_SKILL_VERSION_CHARS
            // **The characters matter, and a length check alone was not enough.** `render_procedure`
            // interpolates this value into a rendered body, so a version containing a newline could forge a
            // step — the text would read as two steps, one of which the author never wrote. The set is the one
            // `ToolId::validate_version` accepts, restated because this crate cannot depend on `jarvis-tools`
            // and stated here so a version is validated by what it may contain rather than by how long it is.
            //
            // This was a real hole, not a hypothetical: the rule was length-only in `SkillStep::new`, only
            // length in `0010`'s `CHECK`, and the migration's own comment claimed the two "cannot disagree
            // about what a version looks like" while neither looked at a single character.
            || !tool_version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
        {
            return Err(InvalidSkill::ToolVersion);
        }
        let instruction = instruction.into();
        let instruction = instruction.trim();
        if instruction.is_empty() || instruction.chars().count() > MAX_SKILL_STEP_CHARS {
            return Err(InvalidSkill::StepText);
        }
        Ok(Self {
            position,
            tool,
            tool_version,
            instruction: instruction.to_owned(),
        })
    }

    /// Returns this step's position in the procedure, from 1.
    #[must_use]
    pub const fn position(&self) -> u16 {
        self.position
    }

    /// Returns the tool this step names.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Returns the tool version the revision was authored against.
    ///
    /// Recorded so "which version of the tool did this procedure address" is answerable from the record
    /// (`ADR-0117` §5), rather than reconstructed from the identifier and today's registry.
    #[must_use]
    pub fn tool_version(&self) -> &str {
        &self.tool_version
    }

    /// Returns the step's own instruction.
    ///
    /// This is content a model may read, and it is the text that makes a skill a prompt-injection surface.
    /// It is therefore returned as a plain `&str` for a caller that must **fence** it before a model sees
    /// it, which `ADR-0117` §3 requires and this module cannot enforce — a value cannot know where it is
    /// rendered.
    #[must_use]
    pub fn instruction(&self) -> &str {
        &self.instruction
    }
}

/// The inputs a skill revision is built from.
///
/// A struct rather than positional parameters, and without `Default`, so every field is stated at the call
/// site — the shape `MemoryRecordParts` and `ApprovalRequestParts` use for the same reason.
#[derive(Clone, Debug)]
pub struct SkillRevisionParts {
    /// The skill this revision belongs to, stable across revisions.
    pub skill_id: SkillId,
    /// The workspace that owns it, which is also the boundary retrieval may not cross.
    pub workspace_id: WorkspaceId,
    /// The revision's own identifier, distinct from the skill's.
    ///
    /// A revision is what is stored and audited, so it needs an identity of its own: "which revision of
    /// this procedure ran" must be answerable, and a version *string* is content the author chose rather
    /// than a value the platform issued.
    pub revision_id: SkillId,
    /// The author's version string, which is content rather than identity.
    pub version: String,
    /// What the procedure is for, in prose. Reaches a model, so it is fenced by the caller.
    pub description: String,
    /// The ordered steps.
    pub steps: Vec<SkillStep>,
    /// Where this revision came from. **Required**: a procedure with no provenance is not a skill.
    pub source: MemorySource,
    /// How widely this procedure may be disclosed.
    ///
    /// # Why a procedure needs a classification
    ///
    /// The same reason a claim does, and selection makes it load-bearing: a skill's prose and step
    /// instructions reach a model, so a procedure describing how to handle a confidential workflow must not
    /// be sent to a third-party model. [`crate::SkillQuery`] refuses a revision above the destination's
    /// ceiling, and [`crate::assemble_context`] applies the same rule through a [`crate::ContextItem`].
    ///
    /// Supplied rather than derived, because nothing in the record implies it: the author decides whether
    /// the procedure's *content* is sensitive, the same way a memory's classifier decides for a claim.
    pub sensitivity: Sensitivity,
    /// What state the revision starts in.
    ///
    /// Supplied rather than derived, because the derivation depends on the **author** and not on anything
    /// in the record: a user-authored skill is usable immediately and an agent-authored one is not. The
    /// rule is enforced by [`SkillRevision::new`] against the source's kind, so a caller cannot mark a
    /// model-authored procedure active.
    pub state: SkillState,
    /// The revision this one replaces, for a declared correction.
    ///
    /// `ADR-0117` §5 and `ADR-0045`: replacement is **declared**, never inferred, so which procedure ran is
    /// readable from the record rather than reconstructed by comparing text.
    pub supersedes: Option<SkillId>,
    /// The external fields this revision's source document carried and this platform refused.
    pub dropped_fields: Vec<SkillDroppedField>,
    /// The run that produced it, when one did.
    pub run_id: Option<crate::id::RunId>,
    /// The actor that produced it.
    pub created_by_actor_id: String,
    /// The correlation identity shared with the originating request.
    pub correlation_id: crate::id::CorrelationId,
    /// When it was recorded.
    pub created_at: UtcTimestamp,
}

/// One stored skill revision: a procedure with provenance, a version, and a state.
///
/// Immutable after construction except through the transitions this module exposes — `promote`, `archive`,
/// and `restore` — each a named act rather than a field assignment, so "how did this skill change" is
/// answerable from the call sites that could have changed it. The same shape [`MemoryRecord`] uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillRevision {
    skill_id: SkillId,
    workspace_id: WorkspaceId,
    revision_id: SkillId,
    version: String,
    description: String,
    steps: Vec<SkillStep>,
    source: MemorySource,
    sensitivity: Sensitivity,
    state: SkillState,
    supersedes: Option<SkillId>,
    superseded_by: Option<SkillId>,
    dropped_fields: Vec<SkillDroppedField>,
    /// Who promoted this revision, when someone did.
    ///
    /// Present only after a promotion, and **required** by [`Self::promote`], because `ADR-0117` §4 makes
    /// promotion a durable attributable decision. A revision that became active without this field set
    /// would be one nobody can be asked about.
    promoted_by_actor_id: Option<String>,
    /// When it was promoted.
    promoted_at: Option<UtcTimestamp>,
    run_id: Option<crate::id::RunId>,
    created_by_actor_id: String,
    correlation_id: crate::id::CorrelationId,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
}

impl SkillRevision {
    /// Validates and records a skill revision.
    ///
    /// # Errors
    ///
    /// - [`InvalidSkill::Description`] for empty, whitespace-only, or oversized prose.
    /// - [`InvalidSkill::Steps`] for no step or more than [`MAX_SKILL_STEPS`].
    /// - [`InvalidSkill::Version`] for an unusable version string.
    /// - [`InvalidSkill::Source`] when a **model-authored** source is marked trustworthy, which is
    ///   `ADR-0117` §3's rule.
    /// - [`InvalidSkill::ModelAuthoredTrust`] when a `ModelInference` source carries `Authoritative`
    ///   trust — the specific case of the above that is easiest to get wrong.
    /// - [`InvalidSkill::ModelAuthoredTrust`] when a **model-authored** revision claims to be `Active`,
    ///   which is `ADR-0117` §4's rule: a proposal becomes usable only through a promotion.
    /// - [`InvalidSkill::DuplicateStepPosition`] when two steps share a position.
    /// - [`InvalidSkill::SelfReference`] when the revision supersedes itself or the skill is its own
    ///   revision.
    /// - [`InvalidSkill::DroppedFields`] for too many recorded drops.
    pub fn new(parts: SkillRevisionParts) -> Result<Self, InvalidSkill> {
        Self::build(parts, true)
    }

    /// The one constructor, with the promotion requirement as a parameter.
    ///
    /// # Why the requirement is a parameter
    ///
    /// `new` requires that a **model-authored** revision is not `Active`, because promotion is an
    /// approval. A **decoded** row must be able to hold an active model-authored revision — that is what
    /// a promotion produces — so the check is skipped for a decode and the stored state is applied.
    ///
    /// It is a boolean rather than a second constructor because every **other** rule must stay in one
    /// place: the alternative shapes are worse, since a public flag would let a caller build the record
    /// the rule forbids, and a copied body would be a second copy of the provenance and step rules. The
    /// same shape [`MemoryRecord`](crate::MemoryRecord)'s `build` uses for its entity requirement.
    fn build(parts: SkillRevisionParts, require_promotion: bool) -> Result<Self, InvalidSkill> {
        let SkillRevisionParts {
            skill_id,
            workspace_id,
            revision_id,
            version,
            description,
            steps,
            source,
            sensitivity,
            state,
            supersedes,
            dropped_fields,
            run_id,
            created_by_actor_id,
            correlation_id,
            created_at,
        } = parts;

        let description = description.trim().to_owned();
        if description.is_empty() || description.chars().count() > MAX_SKILL_DESCRIPTION_CHARS {
            return Err(InvalidSkill::Description);
        }
        if version.is_empty() || version.chars().count() > MAX_SKILL_VERSION_CHARS {
            return Err(InvalidSkill::Version);
        }
        if steps.is_empty() || steps.len() > MAX_SKILL_STEPS {
            return Err(InvalidSkill::Steps);
        }
        if dropped_fields.len() > MAX_SKILL_DROPPED_FIELDS {
            return Err(InvalidSkill::DroppedFields);
        }
        if supersedes == Some(skill_id) || revision_id == skill_id {
            return Err(InvalidSkill::SelfReference);
        }

        // Positions must be distinct. A duplicate is refused rather than resolved, because "which of these
        // two runs first" would otherwise depend on the order a collection happened to iterate — the defect
        // `jarvis_mcp`'s catalog refuses when a collision is resolved by configuration order.
        let mut positions: BTreeSet<u16> = BTreeSet::new();
        for step in &steps {
            if !positions.insert(step.position()) {
                return Err(InvalidSkill::DuplicateStepPosition);
            }
        }
        // Stored in position order, so the sequence is the record's order rather than the caller's. A
        // procedure whose step order came from insertion would be a different procedure depending on how it
        // was assembled.
        let mut steps = steps;
        steps.sort_by_key(SkillStep::position);

        // **The provenance rule, which is `ADR-0117` §3.** A model-authored procedure is `Derived` at
        // best: it re-enters a later prompt, so if it could claim authoritative trust it would be read as
        // the user's own instruction — the self-feeding loop the inference boundary exists to prevent.
        if source.kind().is_model_produced() && source.trust() == crate::MemoryTrust::Authoritative
        {
            return Err(InvalidSkill::ModelAuthoredTrust);
        }

        // **The promotion rule, which is `ADR-0117` §4.** An agent that could author a procedure *and*
        // promote it would have authored its own effect. So a model-authored revision may not be recorded
        // active except by a decode, which is how a promotion is read back.
        if require_promotion && source.kind().is_model_produced() && state == SkillState::Active {
            return Err(InvalidSkill::ModelAuthoredTrust);
        }

        Ok(Self {
            skill_id,
            workspace_id,
            revision_id,
            version,
            description,
            steps,
            source,
            sensitivity,
            state,
            supersedes,
            superseded_by: None,
            dropped_fields,
            promoted_by_actor_id: None,
            promoted_at: None,
            run_id,
            created_by_actor_id,
            correlation_id,
            created_at,
            updated_at: created_at,
        })
    }

    /// Rebuilds a revision from a stored row, **re-applying every rule a stored row can state**.
    ///
    /// # Why this exists beside `new` rather than instead of it
    ///
    /// One rule cannot be enforced on a decode: **promotion is an approval**, so `new` refuses a
    /// model-authored revision recorded `Active`. A promoted revision legitimately decodes as active — that
    /// is exactly what a promotion produces — so a decode must not apply that check, or the row a promotion
    /// wrote would be unreadable.
    ///
    /// The gap that opens is closed by the **schema** rather than here: `0010_skill_revisions.sql` requires
    /// that an active model-authored row names its approver, so a row that became active some other way is
    /// refused at write and cannot exist to be decoded. That is the one place the rule can live, and this
    /// constructor's own documentation is where a reader learns the check has a home.
    ///
    /// **Every other rule still applies**, which is the point: a row whose provenance contradicts its kind,
    /// whose steps name an invalid tool, or whose description is blank is refused here as well as at write
    /// time — the pattern [`MemoryRecord`](crate::MemoryRecord)'s `from_stored` establishes.
    ///
    /// # Errors
    ///
    /// Returns every [`InvalidSkill`] `new` returns **except** [`InvalidSkill::ModelAuthoredTrust`] for an
    /// active model-authored revision, which this constructor permits for the reason above.
    pub fn from_stored(parts: SkillRevisionParts) -> Result<Self, InvalidSkill> {
        Self::build(parts, false)
    }

    /// Returns the skill this revision belongs to.
    #[must_use]
    pub const fn skill_id(&self) -> SkillId {
        self.skill_id
    }

    /// Returns the workspace that owns it.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// Returns this revision's own identifier.
    #[must_use]
    pub const fn revision_id(&self) -> SkillId {
        self.revision_id
    }

    /// Returns the author's version string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the procedure's prose.
    ///
    /// Returned as a plain `&str` for a caller that must **fence** it (`ADR-0049`) before a model sees it.
    /// `ADR-0117` §3 requires that, and this module cannot enforce it: a value cannot know where it is
    /// rendered, so the obligation is at the call site that assembles a prompt.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the steps, in position order.
    #[must_use]
    pub fn steps(&self) -> &[SkillStep] {
        &self.steps
    }

    /// Returns the provenance.
    #[must_use]
    pub const fn source(&self) -> &MemorySource {
        &self.source
    }

    /// Returns how widely this procedure may be disclosed.
    #[must_use]
    pub const fn sensitivity(&self) -> Sensitivity {
        self.sensitivity
    }

    /// Returns whether this revision is visible in a workspace.
    ///
    /// The retrieval boundary, stated once here so selection and any future reader share one rule rather
    /// than comparing identifiers at each call site — the same shape [`crate::MemoryRecord`] uses.
    #[must_use]
    pub fn is_visible_in(&self, workspace_id: WorkspaceId) -> bool {
        self.workspace_id == workspace_id
    }

    /// Returns the state.
    #[must_use]
    pub const fn state(&self) -> SkillState {
        self.state
    }

    /// Returns whether a skill may be offered for use.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.state.is_usable()
    }

    /// Returns the revision this one replaced, in a declared correction.
    #[must_use]
    pub const fn supersedes(&self) -> Option<SkillId> {
        self.supersedes
    }

    /// Returns the revision that replaced this one, once one has.
    #[must_use]
    pub const fn superseded_by(&self) -> Option<SkillId> {
        self.superseded_by
    }

    /// Returns the external fields this revision's document carried and this platform refused.
    ///
    /// `ADR-0117` §7 requires the drop to be **recorded** rather than silently ignored, so a reader can
    /// tell the format's intent from this platform's behaviour.
    #[must_use]
    pub fn dropped_fields(&self) -> &[SkillDroppedField] {
        &self.dropped_fields
    }

    /// Returns who promoted this revision, when someone did.
    #[must_use]
    pub fn promoted_by_actor_id(&self) -> Option<&str> {
        self.promoted_by_actor_id.as_deref()
    }

    /// Returns when it was promoted.
    #[must_use]
    pub const fn promoted_at(&self) -> Option<UtcTimestamp> {
        self.promoted_at
    }

    /// Promotes a proposal to active, recording **who** decided and when.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::ModelAuthoredTrust`] when the revision is already active, because a second
    /// promotion would silently replace the approver a first decision named — and `ADR-0043` requires a
    /// decision to be attributable, so overwriting one loses that.
    ///
    /// # Why the approver is a parameter rather than a field a caller sets
    ///
    /// `ADR-0117` §4 makes promotion an **approval that names its approver**, so the value cannot be
    /// assigned: a revision that became active without one would be a procedure nobody can be asked about.
    /// The domain refuses an approval whose approver equals its requester
    /// (`jarvis_core::ApprovalRequest`), and this method is the storage-side consequence — the approver
    /// arrives from the decision path, never from the document or the model.
    ///
    /// # Why this is a domain transition rather than a repository update
    ///
    /// "Promotion is a named act" is the property under test. A repository that accepted an arbitrary new
    /// state would make it possible to reach `Active` without a promotion, which is the boundary the state
    /// exists to hold.
    ///
    /// # ⭐⭐ The author of a proposal may not promote it, because that is the boundary `ADR-0117` §4 draws
    ///
    /// The decision's own reasoning is explicit: *"An agent that could author a procedure and promote it
    /// would have authored its own effect."* **The first version of this function did not check that**, so
    /// the rule §4 names had no enforcement anywhere — the construction check refuses a model-authored
    /// revision recorded `Active`, but an author could simply call `promote` to put it there. The rule was
    /// documented and unenforced, which is the `ADR-0035` shape this repository removes.
    ///
    /// The check is a **self-approval** refusal, and it is deliberately the same rule
    /// `jarvis_core::ApprovalRequest` applies to a tool approval: an approver equal to the requester is not
    /// an approval. Comparing the strings is the whole check because both are actor identifiers from one
    /// namespace — the platform's actors are users and runs, and a run never has a user's identifier.
    ///
    /// An **empty** approver is refused for the same family of reason, and the two are separate errors so a
    /// caller can tell "you named nobody" from "you named yourself".
    ///
    /// # Why a user promoting its own proposal is unaffected by this guard
    ///
    /// The guard is unconditional, which raises the question of whether it strands a person. The shapes it
    /// separates are: a **run promoting a proposal it authored** — refused, which is the boundary §4 draws —
    /// and a **user promoting a proposal a run produced** — the ordinary path, and the control in this
    /// function's tests. That pairing is what the guard's scope is chosen for.
    ///
    /// It is **not** a claim that a user-authored revision can never be `Proposed`: `Self::build` requires
    /// promotion only of a *model-produced* revision, so a user-authored `Proposed` revision is
    /// **representable** and its own author could promote it. Rather than assume the creation path never
    /// produces that shape, the rule is enforced where it can be: an active model-authored revision must name
    /// a promoter (the schema's `CHECK`), and that promoter must not be the author (this function). A
    /// user-authored procedure needs no promotion to be usable, so a `Proposed` one is an unused shape rather
    /// than an unsafe one — and `promote` refusing an already-`Active` revision means the common case never
    /// reaches this code at all.
    /// ⭐⭐ **A promotion acts on a PROPOSAL, and the first version of this function got that wrong twice.**
    ///
    /// It refused `Active` with [`InvalidSkill::ModelAuthoredTrust`] — the provenance error, about a
    /// *model-authored* revision being recorded active — which is a different rule with a different remedy,
    /// so an operator promoting an already-active revision was told their procedure's provenance was wrong.
    /// And it did **not** refuse `Archived` at all, so an archived revision could be promoted straight back to
    /// `Active`, bypassing [`Self::restore`] — which is the transition that exists for exactly that, and whose
    /// result depends on the promotion record precisely so the two cannot disagree.
    ///
    /// The guard is now on the **source state**, which is the fact the transition is about: only a `Proposed`
    /// revision can be promoted. `StateNotPromotable` names that, and the two former behaviours are both gone
    /// rather than special-cased.
    ///
    /// # Errors
    ///
    /// - [`InvalidSkill::WrongState`] when the revision is not a proposal.
    /// - [`InvalidSkill::PromotionUnattributed`] when the promotion names no approver.
    /// - [`InvalidSkill::ApproverTooLong`] when the approver cannot be stored.
    /// - [`InvalidSkill::PromotionSelfApproval`] when the approver is the revision's own author.
    pub fn promote(
        &self,
        approver_actor_id: impl Into<String>,
        at: UtcTimestamp,
    ) -> Result<Self, InvalidSkill> {
        if self.state != SkillState::Proposed {
            return Err(InvalidSkill::WrongState {
                transition: "be promoted",
                state: self.state,
            });
        }
        let approver = approver_actor_id.into();
        let approver = approver.trim();
        if approver.is_empty() {
            // A promotion without a name is exactly the unattributable decision `ADR-0043` forbids, so it is
            // refused rather than stored.
            return Err(InvalidSkill::PromotionUnattributed);
        }
        // Length is checked because the approver is stored in a bounded column. The domain must refuse a
        // value the schema would reject, or a promotion the domain called valid would fail on write — the
        // "two lengths for one identity" defect the approvals module removed for `decided_by`.
        if approver.chars().count() > MAX_APPROVER_ID_CHARS {
            return Err(InvalidSkill::ApproverTooLong);
        }
        // **The self-approval guard.** Its own variant rather than a reused one, because the two refusals
        // have different remedies: an unattributed promotion needs an approver named, while this one needs a
        // *different* actor to decide — and an operator told "the approver was empty" when they approved
        // their own proposal would look for a missing field that is not missing.
        if approver == self.created_by_actor_id {
            return Err(InvalidSkill::PromotionSelfApproval);
        }
        let mut promoted = self.clone();
        promoted.state = SkillState::Active;
        promoted.promoted_by_actor_id = Some(approver.to_owned());
        promoted.promoted_at = Some(at);
        promoted.updated_at = at;
        Ok(promoted)
    }

    /// Sets a skill aside, retaining it for audit.
    ///
    /// Reversible by [`Self::restore`], which is what makes this different from deletion. A promotion's
    /// attribution is **preserved**: a revision that was once active records who made it so, even after it
    /// is archived, because "who approved this at the time" is the audit question.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::SelfReference`] when the revision is already archived, so a repeated archive
    /// is reported rather than silently accepted — the rule `P3-006c` applied to a redundant transition.
    pub fn archive(&self, at: UtcTimestamp) -> Result<Self, InvalidSkill> {
        if self.state == SkillState::Archived {
            return Err(InvalidSkill::WrongState {
                transition: "be archived",
                state: self.state,
            });
        }
        let mut archived = self.clone();
        archived.state = SkillState::Archived;
        archived.updated_at = at;
        Ok(archived)
    }

    /// Restores an archived revision to the state it held before.
    ///
    /// # ⭐⭐ The derivation has THREE inputs, and the first version used one — which stranded a user's own
    /// procedure
    ///
    /// The state a restore returns to is the one the revision held **before** archiving. Deriving it from the
    /// promotion record rather than from a stored "previous state" field is right: a stored pre-archive state
    /// and the promotion record would be two statements of one fact.
    ///
    /// **But the promotion record alone does not determine it.** It answers "was this approved", and a
    /// revision is usable without ever having been approved when the **person authored it** — so a
    /// user-authored revision archived and restored came back as `Proposed` in the first version. That is not
    /// merely wrong: it is **unfixable through the API**, because promotion compares the approver against the
    /// author and the author is that same user, so no actor could promote it back. A disable/enable pair would
    /// have permanently destroyed a procedure the user wrote.
    ///
    /// So the state is derived from the same three facts that decide it at construction: a model-produced
    /// revision is usable only if it was promoted, and a user-authored one is usable without a promotion. The
    /// two questions are separate, which is why one of them alone cannot answer.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::WrongState`] when the revision is not archived, because restoring something
    /// that was never set aside would silently be a no-op that reports success.
    pub fn restore(&self, at: UtcTimestamp) -> Result<Self, InvalidSkill> {
        if self.state != SkillState::Archived {
            return Err(InvalidSkill::WrongState {
                transition: "be restored",
                state: self.state,
            });
        }
        let mut restored = self.clone();
        restored.state =
            if self.promoted_by_actor_id.is_some() || !self.source.kind().is_model_produced() {
                SkillState::Active
            } else {
                SkillState::Proposed
            };
        restored.updated_at = at;
        Ok(restored)
    }

    /// Marks this revision as replaced, pointing at its successor.
    ///
    /// The `superseded_by` half of a declared correction, set after the successor exists. `ADR-0045`'s
    /// rule is that a correction is **declared** rather than inferred, so this is the second leg of an
    /// explicit pair rather than a comparison of text.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::SelfReference`] when the successor is this revision, which would make the
    /// replacement chain a cycle nothing could follow. Refused rather than stored because a cycle is not
    /// observable until something tries to walk it, and by then the row is already written.
    pub fn superseded_by_revision(&self, successor: SkillId) -> Result<Self, InvalidSkill> {
        if successor == self.revision_id {
            return Err(InvalidSkill::SelfReference);
        }
        let mut replaced = self.clone();
        replaced.superseded_by = Some(successor);
        Ok(replaced)
    }

    /// Returns when the revision was recorded.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }

    /// Returns when the revision was last changed.
    #[must_use]
    pub const fn updated_at(&self) -> UtcTimestamp {
        self.updated_at
    }

    /// Returns the run that produced it, when one did.
    #[must_use]
    pub const fn run_id(&self) -> Option<crate::id::RunId> {
        self.run_id
    }

    /// Returns the actor that recorded it.
    #[must_use]
    pub fn created_by_actor_id(&self) -> &str {
        &self.created_by_actor_id
    }

    /// Returns the correlation identity shared with the originating request.
    #[must_use]
    pub const fn correlation_id(&self) -> crate::id::CorrelationId {
        self.correlation_id
    }

    /// Applies the lifecycle facts a **stored row** holds that construction does not derive.
    ///
    /// # Why this exists beside `new` rather than as fields on the parts
    ///
    /// A caller does not supply these: a promotion writes `promoted_by` and `promoted_at`, a supersession
    /// writes `superseded_by`, and `updated_at` differs from `created_at` for any row that has changed. They
    /// are read from a row, so they arrive in a separate step — the same split
    /// [`MemoryRecord`](crate::MemoryRecord)'s [`StoredMemoryState`](crate::StoredMemoryState) makes.
    ///
    /// Putting them on [`SkillRevisionParts`] would be worse: a caller could then *construct* a revision
    /// claiming it was promoted by someone, which is the unattributable promotion `ADR-0043` forbids. The
    /// only way to reach this state is a decode, and the only way a row reaches it is a promotion.
    #[must_use]
    pub fn with_stored_lifecycle(
        mut self,
        promoted_by_actor_id: Option<String>,
        promoted_at: Option<UtcTimestamp>,
        superseded_by: Option<SkillId>,
        updated_at: UtcTimestamp,
    ) -> Self {
        // Trimmed at the same rule `promote` applies, so a decoded row that arrived with a blank approver
        // is normalised to absent rather than retained as a name nobody has. The schema's own CHECK refuses
        // a blank value at write, so this is the decode-side half of one rule.
        self.promoted_by_actor_id = promoted_by_actor_id
            .map(|actor| actor.trim().to_owned())
            .filter(|actor| !actor.is_empty());
        self.promoted_at = promoted_at;
        self.superseded_by = superseded_by;
        self.updated_at = updated_at;
        self
    }
}

/// The most skills one selection may offer.
///
/// A bound, and a deliberately small one. A skill is a **procedure injected into the prompt beside the
/// tool list**, so offering many is not merely expensive: each one is text a model may follow, and a
/// prompt carrying twenty procedures is one where the model has to choose which instructions apply — the
/// ambiguity `ADR-0117` §2 avoids by making a step an ordinary tool request rather than by trusting the
/// prose. The number matches the scale `MAX_DISCOVERY_TOOLS` uses for the tool surface, because the two
/// lists are read together.
pub const MAX_SELECTED_SKILLS: usize = 8;

/// Explains why one skill was not offered.
///
/// # Why a reason rather than a silent drop
///
/// The same rule [`crate::Ineligibility`] follows for a memory: "why was this not used" must have an
/// answer, and an ineligible skill that simply vanished leaves it unanswerable. A skill is a *procedure*,
/// so the question is more pointed than for a claim — an operator who wrote a skill and never sees it
/// offered needs to know whether the workspace, the destination, the state, or its own text excluded it.
///
/// Ordered roughly by severity, so a caller reporting the **first** refusal reports the most specific one:
/// a foreign skill is not a ranking question at all, a disclosure is the most severe outcome, and the
/// caller's own stated requirements come last.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillIneligibility {
    /// The revision belongs to another workspace, which is the boundary no read may cross.
    ForeignWorkspace,
    /// The revision's content is above the destination's ceiling.
    AboveDestination {
        /// The revision's classification.
        sensitivity: Sensitivity,
        /// The destination's ceiling.
        destination: Sensitivity,
    },
    /// The revision is not `Active`, so it is not a procedure anything may use.
    ///
    /// A proposal awaiting promotion, or an archived revision. `ADR-0117` §4 makes promotion the gate, so
    /// this is the rule that gives the state machine its consequence — without it, `state` would be a field
    /// nothing reads.
    NotUsable {
        /// The state it holds.
        state: SkillState,
    },
    /// The revision has been replaced by a declared correction.
    ///
    /// Distinct from [`Self::NotUsable`] because the remedy differs and the fact is different: an operator
    /// looking at a superseded revision should read its successor, while a proposal needs a promotion.
    Superseded,
    /// It matched none of the query's text.
    ///
    /// Reported rather than omitted so a caller asking "was my skill considered" gets an answer. A skill
    /// selection is small and inspectable, so naming the misses is cheap here in a way it would not be for
    /// a memory scan.
    NoTextMatch,
}

impl SkillIneligibility {
    /// Returns the stable snake-case code, for a log or a wire reply.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ForeignWorkspace => "foreign_workspace",
            Self::AboveDestination { .. } => "above_destination",
            Self::NotUsable { .. } => "not_usable",
            Self::Superseded => "superseded",
            Self::NoTextMatch => "no_text_match",
        }
    }
}

impl fmt::Display for SkillIneligibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// One skill that was considered and not offered, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExcludedSkill {
    /// The revision's identifier.
    pub revision_id: SkillId,
    /// The rule that excluded it.
    pub reason: SkillIneligibility,
}

/// What a skill selection is a function of.
///
/// # Why the defaults are deliberately empty rather than permissive
///
/// Unlike [`crate::MemoryQuery`], this query has **no default destination and no default trust floor**,
/// because a skill has no trust field to floor: its provenance constrains what may be *stored* (`ADR-0117`
/// §3) rather than what may be selected. The destination is required because a skill's text reaches a
/// model, and a default destination would mean a caller who did not think about it disclosed every skill
/// to whatever the default named. So the destination is stated at construction and there is no
/// `Default`.
#[derive(Clone, Debug)]
pub struct SkillQuery {
    /// The workspace to search, which is also the retrieval boundary.
    pub workspace_id: WorkspaceId,
    /// The destination's sensitivity ceiling.
    pub destination: Sensitivity,
    /// The free text to match against a revision's prose and step instructions.
    ///
    /// Empty means "no text requirement", which selects every usable revision in the workspace — the
    /// behaviour an inspection surface wants and a *prompt* must not have, which is why the caller that
    /// assembles a prompt supplies text.
    pub text: String,
}

impl SkillQuery {
    /// Builds a query for one workspace and destination.
    ///
    /// # Why the destination has no default
    ///
    /// See the type's documentation: the permissive default for a retrieval is a disclosure, and a skill's
    /// prose and instructions go into a prompt. Making the caller state it is the same choice
    /// [`crate::MemoryQuery`] makes for its *ceiling*, taken one step further because a skill has no trust
    /// field a floor could narrow.
    #[must_use]
    pub fn new(workspace_id: WorkspaceId, destination: Sensitivity) -> Self {
        Self {
            workspace_id,
            destination,
            text: String::new(),
        }
    }

    /// Sets the free text to match.
    #[must_use]
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Applies the eligibility rules, reporting the first that refuses.
    ///
    /// # Errors
    ///
    /// Returns the [`SkillIneligibility`] naming the rule that excluded the revision. The order is chosen
    /// so the reported reason is the most specific, and it is the same order and reasoning
    /// [`crate::MemoryQuery::is_eligible`] uses: workspace first (a foreign revision is not a ranking
    /// question), then the destination (a disclosure is the most severe outcome), then the two state rules,
    /// then the caller's own text requirement.
    ///
    /// # Why the workspace check is stated as one rule
    ///
    /// `docs/architecture/identity-and-workspaces.md` splits "actor authorization" from "workspace and
    /// sharing policy", and this platform's actor authorization *is* workspace membership — every read in
    /// this codebase binds `workspace_id` as its scope, so a second rule with nothing to distinguish it
    /// would be a rule that always passes. Recorded rather than silently merged, because a reader looking
    /// for the document's two rules should find out why there is one.
    pub fn is_eligible(&self, revision: &SkillRevision) -> Result<(), SkillIneligibility> {
        if !revision.is_visible_in(self.workspace_id) {
            return Err(SkillIneligibility::ForeignWorkspace);
        }
        if !revision.sensitivity().can_flow_to(self.destination) {
            return Err(SkillIneligibility::AboveDestination {
                sensitivity: revision.sensitivity(),
                destination: self.destination,
            });
        }
        // The state rule, which is what gives `SkillState` its consequence: a proposal is a candidate rather
        // than a procedure, and an archived revision was set aside.
        if !revision.is_usable() {
            return Err(SkillIneligibility::NotUsable {
                state: revision.state(),
            });
        }
        // A replaced revision is refused, and refused *separately* from being unusable: it is active, so the
        // state rule passes, and an operator reading "not usable" would look for a promotion that is not the
        // remedy. `ADR-0117` §5: the replacement is declared, so this is a fact rather than an inference.
        if revision.superseded_by().is_some() {
            return Err(SkillIneligibility::Superseded);
        }
        if !self.text.is_empty() && !matches_text(revision, &self.text) {
            return Err(SkillIneligibility::NoTextMatch);
        }
        Ok(())
    }
}

/// Returns whether a revision's text mentions every word of the query.
///
/// # Why whole-word containment and not a ranking signal
///
/// A skill selection is a **prompt-injection decision**, so the question is not "which of these is most
/// relevant" but "did this procedure actually match what was asked for". A fuzzy or weighted match would
/// offer a procedure on a partial overlap, and a procedure that matches loosely is one whose *steps* the
/// model may follow when they do not apply — the failure `ADR-0117` is most concerned with. So the rule is
/// conjunctive and literal: every word of the query must appear, which is what [`crate::MemorySearchKey`]
/// does for a claim's deduplication key and for the same reason.
///
/// Case-insensitive because a procedure's prose is natural language, and word-boundary aware so `read` does
/// not match `already` — a substring match would offer a skill for a word that merely contains a query term.
///
/// **The step instructions are searched as well as the prose**, because a procedure's *actions* are where
/// the tool names and the operational words live: a query naming a tool would otherwise miss a skill whose
/// description never mentions it.
#[must_use]
pub fn matches_text(revision: &SkillRevision, text: &str) -> bool {
    let haystack = skill_haystack(revision);
    text.split_whitespace()
        .all(|word| matches_word(&haystack, word))
}

/// The combined text a query matches against: the prose, then each step's instruction.
///
/// Lowercased once here so a caller does not repeat it, and joined with a space so a word cannot be formed
/// across the boundary between the prose and a step.
fn skill_haystack(revision: &SkillRevision) -> String {
    // Derived from the **rendering** rather than rebuilt field by field. The two were separate
    // concatenations of the same fields, and both omitted the tool identifier — so `matches_word`'s
    // documented property (a query naming a tool finds a procedure that calls it) was untrue, silently, for
    // as long as the omission existed. Deriving one from the other makes that unrepresentable.
    render_procedure(revision).to_lowercase()
}

/// Returns whether one **query word** is present in the haystack.
///
/// # Why a query word is itself split
///
/// A query word may carry separators, and the case that matters most does: `jarvis.files.read` is **one**
/// whitespace-delimited word, and comparing it whole against the haystack's tokens would never match — the
/// haystack holds `jarvis`, `files`, and `read`. So a query word's own segments are what must all appear,
/// which is what makes a query naming a tool find a procedure that calls it. This is not laxness: every
/// segment still has to be present on a word boundary, so the conjunction is preserved one level down.
fn matches_word(haystack: &str, word: &str) -> bool {
    let segments = segments_of(word);
    !segments.is_empty()
        && segments
            .iter()
            .all(|segment| contains_token(haystack, segment))
}

/// Splits a query word into its lowercased alphanumeric segments.
fn segments_of(word: &str) -> Vec<String> {
    word.split(|character: char| !character.is_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Returns whether the haystack contains a token exactly equal to `needle`.
///
/// The haystack is split on any character that is not alphanumeric, so `jarvis.files.read` in a step's
/// instruction contributes the three tokens `jarvis`, `files`, and `read`.
fn contains_token(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|character: char| !character.is_alphanumeric())
        .any(|token| token == needle)
}

/// One skill offered for use, and why it was selected.
///
/// Carries the **revision** rather than a copy of its text, so a caller that must fence the content
/// (`ADR-0117` §3) does it once at the point of assembly rather than receiving a string that looks like
/// prose. The `reason` is a stored value rather than a generated explanation, for the same reason
/// [`crate::SelectionReason`] is.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedSkill {
    /// The offered revision.
    pub revision: SkillRevision,
    /// Why it was offered.
    pub reason: SkillSelectionReason,
}

/// Why a skill was offered.
///
/// Closed, and stored, for the reason every vocabulary here is: a free-form explanation would be a place
/// for content to enter an audit record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillSelectionReason {
    /// Every word of the query appears in the revision's text.
    FullTextMatch,
    /// The query was empty, so the revision was offered without a text requirement.
    ///
    /// The inspection case. Reported separately because "nothing was asked for" and "everything was found"
    /// are different facts, and a caller displaying a list to an operator needs to know which it has.
    NoTextRequirement,
}

impl SkillSelectionReason {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::FullTextMatch => "full_text_match",
            Self::NoTextRequirement => "no_text_requirement",
        }
    }
}

impl fmt::Display for SkillSelectionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// The result of selecting skills: what was offered, what was refused, and what did not fit.
///
/// The same three-part shape [`crate::MemorySelection`] uses, so a caller has one vocabulary for "why is
/// this here", "why is this not", and "why did the list stop".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillSelection {
    /// The offered revisions, most recent first.
    pub offered: Vec<SelectedSkill>,
    /// Every revision considered and refused, with the rule that refused it.
    pub excluded: Vec<ExcludedSkill>,
    /// The revisions that were eligible but did not fit the bound, so a truncation is visible rather than
    /// looking like the end of the list.
    pub dropped: Vec<SkillId>,
    /// How many eligible revisions there were, whether or not they fit.
    pub eligible_total: usize,
}

impl SkillSelection {
    /// Returns whether the list was cut short.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        !self.dropped.is_empty()
    }
}

/// Selects the skills a query may offer, most recent first, dropping the ineligible.
///
/// # Why there is no graded relevance score, and what replaced it
///
/// The first version of this function ranked by **how many query words a revision matched**, and the design
/// was incoherent: eligibility already requires **every** query word to appear ([`matches_text`]), so every
/// offered skill matches the whole query and every score is identical. A ranking whose input is constant
/// cannot order anything, and the only reason it looked like a ranking was that the test exercising it used
/// a query a *candidate* did not fully match — which the eligibility rule refuses outright, so the case
/// could not arise through this function at all. **A filter that is conjunctive cannot also be graded.**
///
/// What the ordering answers instead is the question a caller actually has when two procedures both apply:
/// *which one is current*. So revisions are ordered by **creation instant, newest first**, with the
/// identifier as a tie-break so the order is total and stable across runs — without which two revisions
/// created in the same instant would order by the candidate set's arrival, which for a database-backed read
/// is a query plan away from changing between builds.
///
/// A memory is ranked by nine weighted signals, and that remains right there: a claim's eligibility is a
/// set of *floors* rather than a conjunction, so "which claim is most relevant" is a genuinely graded
/// question. The two retrievals differ because their questions differ.
#[must_use]
pub fn select_skills(revisions: &[SkillRevision], query: &SkillQuery) -> SkillSelection {
    let mut excluded = Vec::new();
    let mut eligible = Vec::new();
    for revision in revisions {
        match query.is_eligible(revision) {
            Ok(()) => {
                let reason = if query.text.is_empty() {
                    SkillSelectionReason::NoTextRequirement
                } else {
                    SkillSelectionReason::FullTextMatch
                };
                eligible.push(SelectedSkill {
                    revision: revision.clone(),
                    reason,
                });
            }
            Err(reason) => excluded.push(ExcludedSkill {
                revision_id: revision.revision_id(),
                reason,
            }),
        }
    }

    let eligible_total = eligible.len();
    // Newest first, then by identifier ascending, so the order is total and stable.
    eligible.sort_by(|left, right| {
        right
            .revision
            .created_at()
            .unix_nanos()
            .cmp(&left.revision.created_at().unix_nanos())
            .then_with(|| {
                left.revision
                    .revision_id()
                    .cmp(&right.revision.revision_id())
            })
    });

    // Truncation is recorded rather than silent, so a caller can tell "there are no more" from "there were
    // more and they did not fit" — the same distinction `DiscoveryReport` reports by count.
    let dropped: Vec<SkillId> = eligible
        .iter()
        .skip(MAX_SELECTED_SKILLS)
        .map(|selected| selected.revision.revision_id())
        .collect();
    eligible.truncate(MAX_SELECTED_SKILLS);

    SkillSelection {
        offered: eligible,
        excluded,
        dropped,
        eligible_total,
    }
}

/// The estimated token cost of offering one skill, for a context budget.
///
/// # Why an estimate from length rather than a real tokenizer
///
/// The platform has no tokenizer in the domain, and adding one would make this crate depend on a model's
/// vocabulary — which inverts the boundary `ADR-0004` draws. So the estimate is a **character count divided
/// by a divisor**, and the divisor is at the low end of what natural-language tokenizers achieve
/// (roughly four characters per token for English), so the estimate **over-counts** rather than
/// under-counts. Over-counting evicts a skill that might have fit; under-counting overflows the budget,
/// which is the direction that produces a prompt the model cannot receive.
///
/// # It measures `render_procedure`, and only that
///
/// The count is of the string that is actually sent, because an estimate of a *different* string is what lets
/// a budget be exceeded: the first version counted the prose and the instructions and **not** the tool
/// identifiers, so the rendered text was longer than the estimate said and a skill that reported itself as
/// fitting could overflow. Measuring the rendering itself leaves no length arithmetic to get wrong, and
/// `skill_selection`'s own token test pins the two together.
#[must_use]
pub fn estimate_skill_tokens(revision: &SkillRevision) -> u32 {
    /// Characters per token, at the **low** end of the usual range so the estimate over-counts.
    const CHARACTERS_PER_TOKEN: usize = 3;

    let characters = render_procedure(revision).chars().count();
    // At least one token, because `ContextItem` refuses a zero estimate and a non-empty revision always
    // costs something to render.
    u32::try_from(characters.div_ceil(CHARACTERS_PER_TOKEN))
        .unwrap_or(u32::MAX)
        .max(1)
}

/// Renders a procedure as the text a model reads.
///
/// # Why this lives in the domain rather than in the caller that builds the prompt
///
/// `P4-012` is "every step is an **ordinary tool request**", and a step is a tool at a version. The first
/// renderer concatenated the prose and each step's *instruction* and **dropped the tool**, so a procedure
/// reached the model as a numbered list of intentions with no way to perform any of them — the model had to
/// guess which tool a step meant, which is the one thing a procedure exists to say. It was load-bearing in
/// three places at once, all silent:
///
/// 1. The prompt had no tool name, so the procedure was unusable as a procedure.
/// 2. [`estimate_skill_tokens`] counted the same fields, so the budget under-measured what it sent.
/// 3. [`skill_haystack`] indexed the same fields, so `matches_word`'s own documented property — "a query
///    naming a tool finds a procedure that calls it" — was false: an objective containing
///    `jarvis.files.read` split into segments (`jarvis`, `files`, `read`) that the haystack did not contain,
///    so the search the sentence described could never match.
///
/// So the rendering is one function, the estimate measures *it*, and the haystack is derived from it: three
/// artefacts that must agree now cannot disagree, and each was previously a separate concatenation of the
/// same fields with the same field missing.
///
/// # The form
///
/// The tool identifier is written in the **exact spelling the model must emit** in a tool call, including its
/// version, so a model can copy it rather than infer it. A step names a tool at a version because a procedure
/// is a pinned sequence (`ADR-0117` §5), and the version is rendered for the same reason the name is: the
/// executor resolves a call against the registry, and a step recorded against a version is a statement about
/// which behaviour the author meant.
#[must_use]
pub fn render_procedure(revision: &SkillRevision) -> String {
    let mut rendered = revision.description().to_owned();
    for step in revision.steps() {
        rendered.push_str("\n- ");
        rendered.push_str(step.tool());
        rendered.push('@');
        rendered.push_str(step.tool_version());
        rendered.push_str(": ");
        rendered.push_str(step.instruction());
    }
    rendered
}

/// Renders one selected skill as a context item, **fenced as derived data**.
///
/// # Why this function exists rather than the caller building an item
///
/// `ADR-0117` §3 requires a skill to reach a prompt as fenced content marked as data, and the two ways to
/// get that wrong are both encoding decisions a caller would have to repeat:
///
/// 1. **The trust class.** A skill is [`ContextTrust::Derived`] or [`ContextTrust::Untrusted`] depending on
///    its **provenance**, and never `User`. Deriving it here from the source kind means a caller cannot
///    label a model-authored procedure as the person's own instruction, and
///    [`ContextSourceKind::Skill`]'s own trust set refuses it if they try.
/// 2. **The step text.** The instructions are what a model would follow, and [`render_procedure`] is the one
///    statement of how they are rendered — prose, then every tool at its version with its instruction. The
///    item's reference names the **revision** (`skill:<id>`), which is provenance and fits the reference's
///    bound; the body is built by the caller from `render_procedure`, because a revision's text is far longer
///    than a reference may be. This doc previously said the step text was carried *in the reference*, which
///    was never true of this function and is the kind of comment that hides a missing field: a reader who
///    trusted it would look for the body in the wrong place.
///
/// # Why `quoted` is always true
///
/// A skill is never instruction-bearing: it is a procedure retrieved from memory, so it is `Derived` or
/// `Untrusted`, and [`ContextItem::new`] requires untrusted content to be marked quoted. Setting it here
/// rather than letting a caller decide is the same reasoning — the flag records that the content is data,
/// and a caller who could clear it would be marking a procedure as the model's own instruction.
///
/// # Errors
///
/// Returns [`crate::context::ContextError`] when the reference or the estimate is unusable. A revision's
/// identifier and a non-zero token estimate both satisfy the item's own rules, so a failure here is an
/// internal inconsistency rather than a caller mistake — and it is reported rather than unwrapped so a
/// repository that somehow produced a zero-length revision fails loudly.
pub fn skill_context_item(
    revision: &SkillRevision,
    priority: crate::ContextPriority,
) -> Result<crate::context::ContextItem, crate::context::ContextError> {
    // The provenance decides the trust class: a model-authored procedure is `Derived`, and one an external
    // document supplied is `Untrusted`. `MemorySourceKind::permitted_trust` is already the table for this, and
    // **`Authoritative` is mapped down to `Derived`** because a skill may never be policy — the one place a
    // skill's rule is *stricter* than a memory's, and the reason this is a projection rather than a
    // re-export of the source's own trust.
    let trust = match revision.source().kind().permitted_trust() {
        crate::MemoryTrust::Authoritative | crate::MemoryTrust::Derived => {
            crate::ContextTrust::Derived
        }
        crate::MemoryTrust::Untrusted => crate::ContextTrust::Untrusted,
    };
    crate::context::ContextItem::new(
        crate::ContextSource::new(
            crate::ContextSourceKind::Skill,
            format!("skill:{}", revision.revision_id()),
        )?,
        trust,
        revision.sensitivity(),
        priority,
        estimate_skill_tokens(revision),
        crate::InclusionReason::RetrievedMatch,
        // Always quoted: a procedure is retrieved content, so it is data rather than instruction.
        true,
    )
}

#[cfg(test)]
mod tests;
