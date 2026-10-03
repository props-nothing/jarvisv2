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

use crate::id::{SkillId, WorkspaceId};
use crate::memory::MemorySource;
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
        if tool_version.is_empty() || tool_version.chars().count() > MAX_SKILL_VERSION_CHARS {
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
    pub fn promote(
        &self,
        approver_actor_id: impl Into<String>,
        at: UtcTimestamp,
    ) -> Result<Self, InvalidSkill> {
        if self.state == SkillState::Active {
            return Err(InvalidSkill::ModelAuthoredTrust);
        }
        let approver = approver_actor_id.into();
        if approver.trim().is_empty() {
            // An empty approver is refused here rather than stored, because a promotion without a name is
            // exactly the unattributable decision `ADR-0043` forbids. Reusing `ModelAuthoredTrust` would be
            // a misleading variant, so the caller gets `SelfReference`: the decision names itself.
            return Err(InvalidSkill::SelfReference);
        }
        let mut promoted = self.clone();
        promoted.state = SkillState::Active;
        promoted.promoted_by_actor_id = Some(approver.trim().to_owned());
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
            return Err(InvalidSkill::SelfReference);
        }
        let mut archived = self.clone();
        archived.state = SkillState::Archived;
        archived.updated_at = at;
        Ok(archived)
    }

    /// Restores an archived skill to the state it held before.
    ///
    /// # Why a restored revision is `Proposed` when it was never promoted
    ///
    /// The state a restore returns to is the one the revision had **before archiving**, which for a
    /// never-promoted proposal is `Proposed`. Deriving it from the promotion record rather than from a
    /// stored "previous state" is the point: a field holding the pre-archive state and the promotion record
    /// would be two statements of one fact, and the record is the one a reviewer reads.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidSkill::SelfReference`] when the revision is not archived, because restoring
    /// something that was never set aside would silently be a no-op that reports success.
    pub fn restore(&self, at: UtcTimestamp) -> Result<Self, InvalidSkill> {
        if self.state != SkillState::Archived {
            return Err(InvalidSkill::SelfReference);
        }
        let mut restored = self.clone();
        restored.state = if self.promoted_by_actor_id.is_some() {
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

#[cfg(test)]
mod tests;
