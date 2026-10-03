//! Versioned REST DTOs for the skill inspection and control surface.
//!
//! `P4-013` and `FR-MEM-005`: "explain memory use and support inspect, correct, forget, export, retention,
//! and deletion", applied to a **stored procedure**. `P4-011` built the records and `P4-012` made one
//! selectable, but before this module the whole vocabulary was reachable only from a test — no route could
//! list a skill, no client could promote one, and nothing could create one at all.
//!
//! # The three rules that shape every type here, which are the memory module's rules
//!
//! **No scope in a request body.** A skill belongs to a workspace, and a caller cannot name one: the
//! workspace comes from the authenticated session, and `deny_unknown_fields` makes an attempt to supply one a
//! `422` rather than an ignored value.
//!
//! **A reference never carries the procedure's text.** A skill's description and its step instructions both
//! reach a prompt, so a listing that returned them would be a second, less careful path into a model's
//! context — the same reasoning that keeps content out of a memory listing. `GET /skills/{id}` returns the
//! text, because a person inspecting their own procedure is the one case where the text is the answer.
//!
//! **A request that changes state names the revision and its counter.** `expected_version` is required on
//! every control verb, because a promotion, a disable, or a deletion applied to a revision the operator has
//! not read is a change to a stored procedure they did not ask for. `ADR-0117` §4 makes promotion an
//! attributable **decision**, and a decision taken against a version the operator never saw is not one.
//!
//! # Why creation is here, and why it is the smallest request in this file
//!
//! `P4-012` recorded "there is no creation surface" as a limit: a skill could only arrive from a test, so the
//! retrieval path had no producer. The request is deliberately thin because **a skill is not a permission** —
//! there is no field for a granted tool, a scope, or an approval to arrive in. A caller states what the
//! procedure says; everything about what it may *do* is decided when a step runs.
//!
//! # Why no request carries an approver for its own author
//!
//! The daemon derives the author from the authenticated session, so a caller cannot name one. For a promotion
//! the approver **is** supplied, because promotion is a decision — and the domain refuses an approver that
//! equals the revision's author, which is the boundary `ADR-0117` §4 draws. That refusal is why the author
//! must come from the session: if the wire could choose the author, it could choose one that differs from the
//! approver and defeat the guard.

use serde::{Deserialize, Serialize};

/// The stable wire name of a skill's state.
pub type SkillStateName = String;

/// One skill revision in a listing: a reference, never the procedure's text.
///
/// # Why the tools are on the reference
///
/// `P4-013` names "what it names" as part of inspection, and a tool identifier is **metadata rather than
/// content**: it is already on the wire in `ToolReply`, it is not prose that reaches a prompt, and it is the
/// fastest way for an operator to recognise which procedure a row is. So the names travel with the reference
/// while the instructions do not.
///
/// # Why `version_counter` is not the author's version
///
/// A skill revision carries **two** version-shaped facts and they must not be confused. `author_version` is
/// the string the author chose (`"2"`), which is content and is not unique across workspaces. `version_counter`
/// is the optimistic-locking value the platform issues, which is what a control verb must present. Naming one
/// `version` would make a client send the author's prose where a counter is expected, and the guard would then
/// refuse every write for a reason that looks like a conflict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReference {
    /// The revision's identifier.
    pub revision_id: String,
    /// The skill this revision belongs to, stable across revisions.
    ///
    /// Distinct from `revision_id` because a revision needs an identity separate from its procedure's — "which
    /// revision of this procedure ran" must be answerable, and a version *string* is content the author chose.
    pub skill_id: String,
    /// The author's version string.
    pub author_version: String,
    /// The state it holds.
    pub state: SkillStateName,
    /// Its classification, which bounds where it may be disclosed.
    pub sensitivity: String,
    /// Where it came from.
    pub source_kind: String,
    /// The source's locator.
    pub source_locator: String,
    /// The tool identifiers the procedure names, which is what a model would be asked to call.
    pub tool_ids: Vec<String>,
    /// When it was recorded, as an RFC 3339 instant.
    pub created_at: String,
    /// When it was last changed.
    pub updated_at: String,
    /// Who promoted it, absent for a revision that never needed a promotion.
    ///
    /// A user-authored revision is active from the outset and no decision made it so, so this is the one field
    /// that distinguishes "the person wrote this" from "an agent proposed it and someone approved".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_by_actor_id: Option<String>,
    /// When it was promoted, absent when it never was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_at: Option<String>,
    /// The revision that replaced it, present only for a superseded revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// The revision this one declares it replaced, for a declared correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The counter a control verb must present.
    ///
    /// # Why this is on the reference rather than only on a write's reply
    ///
    /// The guard requires a counter the caller **observed**, and the caller observes it by reading. A listing
    /// that omitted it would leave a client with no way to obtain the value it must send, so it would re-read
    /// and hope — which is exactly the lost update the guard exists to prevent.
    pub version_counter: i64,
}

/// Reply body for `GET /api/v1/skills`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillListReply {
    /// The revisions, newest first.
    pub skills: Vec<SkillReference>,
    /// How many were returned.
    pub returned: u32,
    /// The bound that was applied, so a caller can tell a complete listing from a truncated one.
    ///
    /// A caller that cannot see the bound reads a truncated list as the whole answer. For "which procedures do
    /// you have" that is a materially wrong answer rather than an incomplete one, which is the same reasoning
    /// the memory listing records.
    pub limit: u32,
}

/// One step, as a person reads it and as a model would be asked to perform it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillStepBody {
    /// Its position in the sequence.
    pub position: u16,
    /// The tool it names.
    pub tool: String,
    /// The tool version the step was written against.
    ///
    /// **Evidence, not a grant.** It records which version the procedure addressed; whether the current grant
    /// still covers the call is decided when the step runs, never when the skill is read (`ADR-0117` §6).
    pub tool_version: String,
    /// What to do.
    pub instruction: String,
}

/// One field a skill's source document carried that this platform **refused**.
///
/// `ADR-0117` §7 requires the drop to be **recorded** rather than silently ignored: a field
/// accepted-then-ignored is worse than one never accepted, because a reader of the stored skill cannot
/// otherwise tell the format's intent from this platform's behaviour. Reporting the drops is what makes the
/// difference visible to a person rather than to a reader of the migration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DroppedFieldBody {
    /// The field's name in the source document.
    pub field: String,
    /// Why it was refused, by its stable category.
    pub reason: String,
    /// Whether the dropped field was **authority-bearing**.
    ///
    /// Separated from the category because it is the fact an operator needs: a dropped comment style is a
    /// formatting difference, while a dropped tool grant or pre-approval is a field that would have widened
    /// what this procedure may do.
    pub authority_bearing: bool,
}

/// Reply body for `GET /api/v1/skills/{id}`, which is the one place the procedure's text is returned.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillDetailReply {
    /// The reference fields.
    #[serde(flatten)]
    pub reference: SkillReference,
    /// The procedure's prose.
    pub description: String,
    /// The ordered steps, which are the procedure's body.
    pub steps: Vec<SkillStepBody>,
    /// The fields its source document carried and this platform refused.
    pub dropped_fields: Vec<DroppedFieldBody>,
    /// Whether this revision may be offered for use right now.
    ///
    /// The single predicate a presentation site should ask, computed by the domain so a client cannot apply
    /// one of its conditions and forget the others: a proposal, an archived revision, and a superseded one are
    /// all "not usable", for three different reasons, and a client deriving that from `state` alone would get
    /// the superseded case wrong.
    pub is_usable: bool,
    /// Why it is not usable, when it is not, by the domain's own reason code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unusable_reason: Option<String>,
}

/// Request body for `POST /api/v1/skills`, which is the creation surface `P4-012` recorded as missing.
///
/// # Why there is no field for authority
///
/// `ADR-0117`'s central rule is that a skill names already-granted tools and carries no authority of its own.
/// So there is no field here for a granted tool set, a scope, a pre-approval, or an approver: nothing accepts
/// authority and there is nowhere one could arrive. The consequence is that a skill's creation **cannot be
/// privilege escalation** — a client that could submit one here gains exactly the ability to write a
/// document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillRequest {
    /// The procedure's prose.
    pub description: String,
    /// The procedure's steps, in the order they should run.
    ///
    /// Positions are supplied rather than inferred from array order, and the domain refuses a duplicate —
    /// "which of these two runs first" must not depend on how a collection happened to iterate. An array order
    /// that silently became the positions would be exactly that defect.
    pub steps: Vec<CreateSkillStep>,
    /// The author's version string.
    pub author_version: String,
    /// The revision this one replaces, for a **declared** correction.
    ///
    /// `ADR-0117` §5 and `ADR-0045`: replacement is declared rather than inferred, so a correction states what
    /// it replaces instead of being recognized by comparing text. A revision that names a predecessor
    /// belonging to a different procedure is refused, because a chain that leaves its own skill cannot answer
    /// "what replaced this".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The skill a correction belongs to, required when `supersedes` is present.
    ///
    /// # Why a correction must state this and a new procedure must not
    ///
    /// A new procedure's identity is issued by the platform, so a caller supplying one could choose an
    /// identifier that collides with something else. A **correction** necessarily belongs to an existing
    /// skill, and the revision it replaces states which — but the domain's guard compares the predecessor's
    /// identifier against the skill's, and neither is derivable without the store. So the caller states the
    /// skill, the daemon confirms the predecessor is one of its revisions, and a mismatch is refused. The
    /// alternative was to read the predecessor and take its skill id silently, which would let a caller point
    /// a correction at a procedure it never named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_id: Option<String>,
}

/// One step of a creation request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillStep {
    /// Its position in the sequence.
    pub position: u16,
    /// The tool it names. The daemon checks the identifier and that the tool is registered.
    pub tool: String,
    /// The tool version the step is written against.
    pub tool_version: String,
    /// What to do.
    pub instruction: String,
}

/// Request body for `POST /api/v1/skills/{id}/promote`.
///
/// # Why the approver is supplied here and nowhere else
///
/// Every other control verb is an operator acting on their own procedure, so the actor comes from the session
/// and need not be stated. Promotion is different: `ADR-0117` §4 makes it an **approval**, and `ADR-0043`
/// requires a decision to name who made it. So the approver is a field, and the daemon refuses one equal to
/// the revision's author — the self-approval boundary, which is only meaningful because the author is derived
/// from the session rather than chosen by the caller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromoteSkillRequest {
    /// The counter the caller observed.
    pub expected_version: i64,
    /// Who approved it.
    pub approver_actor_id: String,
}

/// Request body for the two enable/disable verbs, which carry only the guard they share.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillTransitionRequest {
    /// The counter the caller observed.
    pub expected_version: i64,
}

/// Reply body for a creation or a control verb.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReply {
    /// The reference fields, which carry the counter for a subsequent verb.
    #[serde(flatten)]
    pub reference: SkillReference,
    /// What the operation did, by its stable name.
    pub outcome: String,
    /// Why, when the operation changed what was usable or refused to.
    ///
    /// Present for a refusal and for a promotion, absent for a plain creation. A refusal that says only
    /// "refused" sends the caller looking for a policy problem that may not exist — the rule the memory reply
    /// records, and a promotion is the case here where the reason is worth reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Request body for `POST /api/v1/skills/{id}/forget`.
///
/// # Why there is no `allow_relearn` here, unlike a memory's forget
///
/// A memory's tombstone exists to stop a claim **returning through a later ingest**, because ingestion is
/// continuous and automatic. A skill has no ingest: it is written by an explicit request, so a deletion
/// cannot be undone by a process the user did not run. A tombstone would be a row nothing consults, which is
/// the shape this repository removes — so a skill's deletion is final and the receipt says so rather than
/// offering an option with no consequence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetSkillRequest {
    /// The counter the caller observed.
    pub expected_version: i64,
}

/// Summary of what a skill deletion removed.
///
/// The same name and the same purpose as the memory receipt, and deliberately not the same fields: a receipt
/// counts what was removed by **category** so it is distinguishable from one for a record that never existed,
/// and a skill's categories are its own (steps, drops, an entity link would be meaningless here).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillDeletionReceipt {
    /// The revision that was deleted.
    pub revision_id: String,
    /// The description's length in characters, so a caller can see that text was removed rather than assume it.
    ///
    /// A length and not the text: the receipt is evidence, and evidence that quotes the removed content is the
    /// content surviving the deletion in a second table.
    pub removed_description_chars: u32,
    /// How many steps were removed.
    ///
    /// A count rather than a boolean, because a procedure has an ordered body and "four steps went" is
    /// checkable against what the caller wrote while "steps were handled" is not.
    pub removed_steps: u32,
    /// How many recorded dropped fields went with it.
    pub removed_dropped_fields: u32,
    /// Whether supersession links were cleared.
    ///
    /// Both directions, because a deleted revision may have been replacing another and may itself have been
    /// replaced — and clearing one leg without the other would leave a chain that points at nothing.
    pub cleared_supersession: bool,
    /// What the deletion could not reach, named.
    ///
    /// Never empty, and the first entry is why the field exists: a procedure that was **offered as context**
    /// reached a model, and a provider holds its own copy of any prompt under its own retention policy. This
    /// platform cannot reach that copy, and `docs/architecture/memory-and-context.md` requires it be surfaced
    /// rather than implied by a receipt that reads as total.
    pub unreachable: Vec<String>,
}

/// Reply body for `GET /api/v1/skills/export`.
///
/// # Why the export carries the text
///
/// `FR-MEM-005` names "export" alongside inspection, and an export whose procedure text was redacted would
/// not be one. This is the deliberate exception to the reference-only rule: the user asked for their own data,
/// which is the case portability exists for. The envelope states what it **excludes**, because an archive that
/// looks complete and is not is worse than one that says what it left out.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillExportReply {
    /// The workspace the export covers.
    pub workspace_id: String,
    /// When the export was produced, as an RFC 3339 instant.
    pub exported_at: String,
    /// The revisions, newest first.
    pub skills: Vec<ExportedSkill>,
    /// How many revisions the export contains.
    pub count: u32,
    /// What the export deliberately does not contain.
    pub exclusions: Vec<String>,
}

/// One exported revision, with its text and provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportedSkill {
    /// The reference fields.
    #[serde(flatten)]
    pub reference: SkillReference,
    /// The procedure's prose.
    pub description: String,
    /// The ordered steps.
    pub steps: Vec<SkillStepBody>,
    /// The fields its source document carried and this platform refused.
    pub dropped_fields: Vec<DroppedFieldBody>,
}
