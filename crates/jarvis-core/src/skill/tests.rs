//! Tests for the skill vocabulary, weighted toward the two rules that are security properties.

use super::*;
use crate::MemoryTrust;
use crate::id::{CorrelationId, RunId};
use crate::memory::MemorySourceKind;

/// The validator a step is built with, mirroring `jarvis_tools::ToolId`'s shape.
///
/// A local function rather than the real `ToolId::new`, because this crate cannot depend on
/// `jarvis-tools`. The rule it mirrors is "a dotted identifier with a non-empty namespace and name", which
/// is what the real validator enforces; a test that used a vacuous `|_| true` would not exercise the
/// rejection path at all.
fn valid_tool(identifier: &str) -> bool {
    identifier
        .split_once('.')
        .is_some_and(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
}

fn step(position: u16, tool: &str) -> SkillStep {
    SkillStep::new(
        position,
        tool,
        "1.0.0",
        "Do the thing described.",
        valid_tool,
    )
    .unwrap_or_else(|error| panic!("the fixture step must be accepted: {error}"))
}

fn parts() -> SkillRevisionParts {
    let skill_id = SkillId::new();
    SkillRevisionParts {
        skill_id,
        workspace_id: WorkspaceId::new(),
        revision_id: SkillId::new(),
        version: "1".to_owned(),
        description: "Read the user's notes and summarize the open items.".to_owned(),
        steps: vec![step(1, "jarvis.files.read")],
        source: MemorySource::of_kind(MemorySourceKind::UserStatement, "session-1")
            .unwrap_or_else(|error| panic!("fixture source: {error}")),
        state: SkillState::Active,
        supersedes: None,
        dropped_fields: Vec::new(),
        run_id: Some(RunId::new()),
        created_by_actor_id: "user-1".to_owned(),
        correlation_id: CorrelationId::new(),
        created_at: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_000)
            .unwrap_or_else(|error| panic!("fixture clock: {error}")),
    }
}

fn now() -> UtcTimestamp {
    UtcTimestamp::from_unix_nanos(1_700_000_100_000_000_000)
        .unwrap_or_else(|error| panic!("fixture clock: {error}"))
}

/// A user-authored revision is active and usable.
#[test]
fn a_user_authored_revision_is_active() {
    let revision = SkillRevision::new(parts())
        .unwrap_or_else(|error| panic!("a user-authored skill must be accepted: {error}"));
    assert_eq!(revision.state(), SkillState::Active);
    assert!(revision.is_usable());
    assert_eq!(revision.steps().len(), 1);
    assert_eq!(revision.promoted_by_actor_id(), None);
}

/// **⭐⭐ A model-authored revision cannot claim authoritative provenance, and it is the source that refuses it first.**
///
/// `ADR-0117` §3. A self-authored procedure re-enters a later prompt, so if it could claim authoritative
/// trust it would be read as the user's own instruction — the self-feeding loop the inference boundary
/// exists to prevent.
///
/// **The refusal is at `MemorySource`, which is the first enforcer, and this test asserts that rather than
/// skipping it.** Asserting the skill-level rule here would be unreachable: a `ModelInference` source
/// physically cannot carry `Authoritative` trust, so no such revision can be constructed to test against.
/// Recording that is the honest form of the claim — the rule has two layers and the *inner* one is what
/// makes the outer one unreachable, which is what defence in depth means in practice.
#[test]
fn a_model_authored_revision_cannot_claim_authoritative_provenance() {
    assert_eq!(
        MemorySource::new(
            MemorySourceKind::ModelInference,
            "run-1",
            MemoryTrust::Authoritative,
            None,
        )
        .err(),
        Some(crate::InvalidMemory::Source),
        "a model inference must not be representable as authoritative"
    );
    // And the skill-level rule still fires for the case that IS reachable: a model-produced source claiming
    // active state, which is the promotion rule tested separately below.
}

/// **⭐⭐ A model-authored revision cannot be recorded active: promotion is an approval.**
///
/// `ADR-0117` §4. An agent that could author a procedure *and* promote it would have authored its own
/// effect — precisely the boundary `AGENTS.md` draws. So a model-produced revision must begin `Proposed`,
/// and this asserts that claiming `Active` is refused rather than corrected silently.
#[test]
fn a_model_authored_revision_cannot_be_recorded_active() {
    let mut authored = parts();
    authored.source = MemorySource::of_kind(MemorySourceKind::ModelInference, "run-1")
        .unwrap_or_else(|error| panic!("fixture source: {error}"));
    authored.state = SkillState::Active;

    assert_eq!(
        SkillRevision::new(authored).err(),
        Some(InvalidSkill::ModelAuthoredTrust),
        "a model-authored skill must not be recordable as active"
    );
}

/// **The control for the test above: the same revision is accepted as a proposal.**
///
/// Without this, an implementation that refused every model-authored revision would satisfy the assertion
/// above while making the feature unusable — the complement-asserted-separately rule.
#[test]
fn the_same_model_authored_revision_is_accepted_as_a_proposal() {
    let mut authored = parts();
    authored.source = MemorySource::of_kind(MemorySourceKind::ModelInference, "run-1")
        .unwrap_or_else(|error| panic!("fixture source: {error}"));
    authored.state = SkillState::Proposed;

    let revision = SkillRevision::new(authored).unwrap_or_else(|error| {
        panic!("a proposed model-authored skill must be accepted: {error}")
    });
    assert_eq!(revision.state(), SkillState::Proposed);
    assert!(
        !revision.is_usable(),
        "a proposal must not be offered for use"
    );
}

/// **⭐⭐ Promotion records who decided, and only a promotion reaches `Active`.**
///
/// `ADR-0117` §4 and `ADR-0043`: promotion is a durable, attributable decision. The promotion is a named
/// act rather than a field assignment, so "how did this become active" is answerable from the call sites.
#[test]
fn promotion_records_the_approver_and_is_the_only_route_to_active() {
    let mut proposal_parts = parts();
    proposal_parts.source = MemorySource::of_kind(MemorySourceKind::ModelInference, "run-1")
        .unwrap_or_else(|error| panic!("fixture source: {error}"));
    proposal_parts.state = SkillState::Proposed;
    let proposal = SkillRevision::new(proposal_parts)
        .unwrap_or_else(|error| panic!("fixture proposal: {error}"));

    let promoted = proposal
        .promote("user-1", now())
        .unwrap_or_else(|error| panic!("a proposal must be promotable: {error}"));
    assert_eq!(promoted.state(), SkillState::Active);
    assert_eq!(promoted.promoted_by_actor_id(), Some("user-1"));
    assert_eq!(promoted.promoted_at(), Some(now()));
    assert!(promoted.is_usable());

    // A second promotion is refused: it would silently replace the approver the first decision named.
    assert_eq!(
        promoted.promote("user-2", now()).err(),
        Some(InvalidSkill::ModelAuthoredTrust),
        "a second promotion must not overwrite the approver a decision named"
    );
}

/// **A promotion with a blank approver is refused.**
///
/// An unattributable promotion is exactly what `ADR-0043` forbids, and an empty string is how one would
/// arrive from a caller that read a missing field.
#[test]
fn a_promotion_without_an_approver_is_refused() {
    let mut proposal_parts = parts();
    proposal_parts.state = SkillState::Proposed;
    let proposal = SkillRevision::new(proposal_parts)
        .unwrap_or_else(|error| panic!("fixture proposal: {error}"));
    assert_eq!(
        proposal.promote("   ", now()).err(),
        Some(InvalidSkill::SelfReference),
        "a promotion with no approver must be refused rather than stored"
    );
}

/// **Archive is reversible and preserves the promotion's attribution.**
///
/// Archiving is what makes setting a skill aside different from deleting it, and a restored revision must
/// return to the state the promotion record implies — `Active` when it was promoted, `Proposed` when it was
/// never. Deriving that rather than storing a "previous state" field is what keeps one statement of the
/// fact.
#[test]
fn archiving_is_reversible_and_preserves_attribution() {
    // A **model-authored proposal**, promoted by the user. The fixture starts from a proposal because the
    // promotion is what sets the attribution this test asserts survives archiving; a user-authored revision
    // is already active and cannot be promoted (a second promotion would overwrite the approver).
    let mut proposal_parts = parts();
    proposal_parts.source = MemorySource::of_kind(MemorySourceKind::ModelInference, "run-1")
        .unwrap_or_else(|error| panic!("fixture source: {error}"));
    proposal_parts.state = SkillState::Proposed;
    let active = SkillRevision::new(proposal_parts)
        .unwrap_or_else(|error| panic!("fixture: {error}"))
        .promote("user-1", now())
        .unwrap_or_else(|error| panic!("promote: {error}"));

    let archived = active
        .archive(now())
        .unwrap_or_else(|error| panic!("archive: {error}"));
    assert_eq!(archived.state(), SkillState::Archived);
    assert!(!archived.is_usable());
    assert_eq!(
        archived.promoted_by_actor_id(),
        Some("user-1"),
        "who approved this at the time is the audit question and must survive archiving"
    );

    let restored = archived
        .restore(now())
        .unwrap_or_else(|error| panic!("restore: {error}"));
    assert_eq!(
        restored.state(),
        SkillState::Active,
        "a promoted revision returns to active"
    );

    // A proposal never promoted returns to `Proposed`, which is the other half of the derivation.
    let mut proposal_parts = parts();
    proposal_parts.state = SkillState::Proposed;
    let proposal = SkillRevision::new(proposal_parts)
        .unwrap_or_else(|error| panic!("fixture proposal: {error}"));
    let archived_proposal = proposal
        .archive(now())
        .unwrap_or_else(|error| panic!("archive: {error}"));
    assert_eq!(
        archived_proposal
            .restore(now())
            .unwrap_or_else(|error| panic!("restore: {error}"))
            .state(),
        SkillState::Proposed,
        "a never-promoted revision returns to a proposal, not to active"
    );
}

/// **A redundant transition is refused rather than silently accepted.**
///
/// Archiving an archived revision and restoring a live one are both no-ops that would report success, so
/// they are refused — the `P3-006c` rule that a redundant transition reads as a state change that did not
/// happen.
#[test]
fn redundant_transitions_are_refused() {
    let active = SkillRevision::new(parts()).unwrap_or_else(|error| panic!("fixture: {error}"));
    assert!(
        active.archive(now()).is_ok(),
        "archiving a live skill must work"
    );
    assert_eq!(
        active.restore(now()).err(),
        Some(InvalidSkill::SelfReference),
        "restoring a revision that was never archived must be refused"
    );

    let archived = active
        .archive(now())
        .unwrap_or_else(|error| panic!("archive: {error}"));
    assert_eq!(
        archived.archive(now()).err(),
        Some(InvalidSkill::SelfReference),
        "archiving twice must be refused"
    );
}

/// **⭐⭐ A step's tool version is recorded, so "which procedure ran" is answerable.**
///
/// `ADR-0117` §5 requires a stored procedure to carry the tools it names at that version. The property is
/// audit rather than validation: a later tool revision can change a schema, and the version a procedure was
/// authored against is not reconstructible from the identifier alone.
#[test]
fn a_step_records_the_tool_version_it_was_authored_against() {
    let revision = SkillRevision::new(parts()).unwrap_or_else(|error| panic!("fixture: {error}"));
    let recorded = &revision.steps()[0];
    assert_eq!(recorded.tool(), "jarvis.files.read");
    assert_eq!(recorded.tool_version(), "1.0.0");
    assert_eq!(recorded.position(), 1);
}

/// **A step that names a tool this platform rejects is refused.**
///
/// The validator is `jarvis-tools`'s rule passed in, and the assertion uses an identifier with no
/// namespace/name split — which the real `ToolId` refuses. A vacuous validator would let this through,
/// which is why the fixture validates rather than accepting everything.
#[test]
fn a_step_naming_an_invalid_tool_is_refused() {
    assert_eq!(
        SkillStep::new(1, "not-a-tool", "1.0.0", "Do it.", valid_tool).err(),
        Some(InvalidSkill::ToolIdentifier)
    );
}

/// **Two steps at one position are refused rather than resolved.**
///
/// "Which of these two runs first" would otherwise depend on the order a collection happened to iterate,
/// which is the defect `jarvis_mcp`'s catalog refuses when a name collision is resolved by configuration
/// order.
#[test]
fn duplicate_step_positions_are_refused() {
    let mut duplicated = parts();
    duplicated.steps = vec![step(1, "jarvis.files.read"), step(1, "jarvis.mail.send")];
    assert_eq!(
        SkillRevision::new(duplicated).err(),
        Some(InvalidSkill::DuplicateStepPosition)
    );
}

/// **Steps are stored in position order, not in the order they were supplied.**
///
/// The sequence is the record's order rather than the caller's, so a procedure assembled out of order is
/// the same procedure. Asserted by supplying them reversed.
#[test]
fn steps_are_ordered_by_position() {
    let mut shuffled = parts();
    shuffled.steps = vec![
        step(3, "jarvis.third.step"),
        step(1, "jarvis.first.step"),
        step(2, "jarvis.second.step"),
    ];
    let revision = SkillRevision::new(shuffled).unwrap_or_else(|error| panic!("fixture: {error}"));
    let positions: Vec<u16> = revision.steps().iter().map(SkillStep::position).collect();
    assert_eq!(positions, vec![1, 2, 3]);
}

/// **A dropped external field is recorded with a reason, and authority-bearing reasons are marked.**
///
/// `ADR-0117` §7: a field that would grant authority, select a tool, or pre-approve an effect is dropped
/// **and the drop is recorded**, because a field accepted-then-ignored is worse than one never accepted —
/// a reader cannot tell the format's intent from this platform's behaviour.
#[test]
fn dropped_fields_are_recorded_with_a_reason() {
    let mut with_drops = parts();
    with_drops.dropped_fields = vec![
        SkillDroppedField::new("allowed_tools", DropReason::ToolSelection)
            .unwrap_or_else(|error| panic!("fixture drop: {error}")),
        SkillDroppedField::new("requires_approval", DropReason::PreApproval)
            .unwrap_or_else(|error| panic!("fixture drop: {error}")),
        SkillDroppedField::new("vendor_rating", DropReason::Unrepresented)
            .unwrap_or_else(|error| panic!("fixture drop: {error}")),
    ];
    let revision =
        SkillRevision::new(with_drops).unwrap_or_else(|error| panic!("fixture: {error}"));

    assert_eq!(revision.dropped_fields().len(), 3);
    assert_eq!(revision.dropped_fields()[0].name(), "allowed_tools");
    assert_eq!(
        revision.dropped_fields()[0].reason(),
        DropReason::ToolSelection
    );
    assert!(revision.dropped_fields()[0].reason().is_authority_bearing());
    assert!(
        !revision.dropped_fields()[2].reason().is_authority_bearing(),
        "a merely unrepresented field is not an authority refusal"
    );
}

/// **A blank or oversized dropped-field name is refused.**
///
/// An unnamed drop cannot be reviewed, which defeats the reason the record exists.
#[test]
fn a_blank_dropped_field_name_is_refused() {
    assert_eq!(
        SkillDroppedField::new("  ", DropReason::Unrepresented).err(),
        Some(InvalidSkill::DroppedFields)
    );
    let oversized = "x".repeat(MAX_SKILL_DROPPED_FIELD_NAME_CHARS + 1);
    assert_eq!(
        SkillDroppedField::new(oversized, DropReason::Unrepresented).err(),
        Some(InvalidSkill::DroppedFields)
    );
}

/// **The step bound is enforced, and the bound is the point at which a procedure becomes a workflow.**
#[test]
fn the_step_bound_is_enforced() {
    let mut too_many = parts();
    // **One over the bound**, so this asserts the limit rather than the count: exactly `MAX_SKILL_STEPS`
    // steps must be accepted, which the second half of the test covers by supplying none.
    too_many.steps = (1..=MAX_SKILL_STEPS + 1)
        .map(|position| {
            step(
                u16::try_from(position).unwrap_or(u16::MAX),
                "jarvis.files.read",
            )
        })
        .collect();
    assert_eq!(
        SkillRevision::new(too_many).err(),
        Some(InvalidSkill::Steps)
    );

    let mut none = parts();
    none.steps = Vec::new();
    assert_eq!(SkillRevision::new(none).err(), Some(InvalidSkill::Steps));
}

/// **A revision cannot supersede itself, and a skill cannot be its own revision.**
///
/// Both would make a chain nothing could follow.
#[test]
fn self_references_are_refused() {
    let mut supersedes_self = parts();
    supersedes_self.supersedes = Some(supersedes_self.skill_id);
    assert_eq!(
        SkillRevision::new(supersedes_self).err(),
        Some(InvalidSkill::SelfReference)
    );

    let mut revision_is_skill = parts();
    revision_is_skill.revision_id = revision_is_skill.skill_id;
    assert_eq!(
        SkillRevision::new(revision_is_skill).err(),
        Some(InvalidSkill::SelfReference)
    );
}

/// **`superseded_by_revision` refuses a cycle, and accepts a real successor.**
#[test]
fn supersession_refuses_a_cycle_and_records_a_successor() {
    let revision = SkillRevision::new(parts()).unwrap_or_else(|error| panic!("fixture: {error}"));
    assert_eq!(
        revision
            .superseded_by_revision(revision.revision_id())
            .err(),
        Some(InvalidSkill::SelfReference),
        "a revision cannot be replaced by itself"
    );

    let successor = SkillId::new();
    let replaced = revision
        .superseded_by_revision(successor)
        .unwrap_or_else(|error| panic!("a real successor must be accepted: {error}"));
    assert_eq!(replaced.superseded_by(), Some(successor));
    assert_eq!(
        replaced.supersedes(),
        None,
        "the direction is the successor: the replaced revision points forward"
    );
}

/// **The declaration `ADR-0117` §5 makes: replacement is declared, so both legs are the caller's.**
///
/// This asserts the *absence* of inference. The successor names what it replaced through `supersedes`, and
/// the predecessor names its successor through `superseded_by_revision`; nothing derives either from
/// comparing prose.
#[test]
fn a_declared_correction_carries_both_legs() {
    let original = SkillRevision::new(parts()).unwrap_or_else(|error| panic!("fixture: {error}"));

    let mut successor_parts = parts();
    successor_parts.version = "2".to_owned();
    successor_parts.supersedes = Some(original.skill_id());
    let successor = SkillRevision::new(successor_parts)
        .unwrap_or_else(|error| panic!("fixture successor: {error}"));
    assert_eq!(
        successor.supersedes(),
        Some(original.skill_id()),
        "the successor declares what it replaced"
    );

    let replaced = original
        .superseded_by_revision(successor.revision_id())
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    assert_eq!(replaced.superseded_by(), Some(successor.revision_id()));
}

/// **Every closed set round-trips through its stable name.**
#[test]
fn the_closed_sets_round_trip() {
    for state in SkillState::all() {
        assert_eq!(state.as_str().parse::<SkillState>(), Ok(state));
        let encoded =
            serde_json::to_string(&state).unwrap_or_else(|error| panic!("serialize: {error}"));
        assert_eq!(encoded, format!("\"{}\"", state.as_str()));
    }
    for reason in DropReason::all() {
        assert_eq!(reason.as_str().parse::<DropReason>(), Ok(reason));
        let encoded =
            serde_json::to_string(&reason).unwrap_or_else(|error| panic!("serialize: {error}"));
        assert_eq!(encoded, format!("\"{}\"", reason.as_str()));
    }
}

/// **Only `Active` is usable, stated as a property over the whole set.**
#[test]
fn only_active_is_usable() {
    for state in SkillState::all() {
        assert_eq!(
            state.is_usable(),
            state == SkillState::Active,
            "{state} usability"
        );
        assert!(
            !state.is_terminal(),
            "{state} must not be terminal: archiving is reversible and deletion is a tombstone"
        );
    }
}
