//! The skill surface: what JARVIS can do repeatedly, and the verbs that change it.
//!
//! `P4-011` built the records, `P4-012` made one selectable and wired it into a real prompt, and `P4-013` is
//! this module: the `FR-MEM-005` lifecycle applied to a stored procedure — list, inspect, create, promote,
//! disable, enable, forget, and export. Before it, the whole vocabulary was reachable only from a test: no
//! route could list a skill, nothing could promote one, and **nothing could create one at all**, which
//! `P4-012` recorded as a limit because it left the retrieval path with no producer.
//!
//! # The scope rule, and where it lives
//!
//! Every operation takes the workspace from a **loaded** `LocalIdentity`, never from a request — the same rule
//! the memory and tool surfaces follow, and what makes "client A's procedure cannot enter client B's prompt" a
//! transport property rather than a check each handler remembers. This deployment has one seeded local
//! workspace, so today the rule cannot fail; it is written this way so it still holds when sharing arrives.
//!
//! # Why the author comes from the session rather than the request
//!
//! A skill's author is the **actor** `ADR-0117` §4's self-approval refusal compares an approver against. If the
//! wire could choose the author, a caller could name one that differs from its approver and defeat that guard —
//! so the author is derived here and a promotion's approver is the only identity a client supplies. The two are
//! then compared where they mean something, in the domain.
//!
//! # Why creation validates tools against the registry
//!
//! A step names a tool, and a procedure naming a tool this deployment does not have is a procedure that cannot
//! run. The **structural** rule (a dotted identifier) is the domain's and the schema's; the **membership** rule
//! needs the registry, so it is applied here — where the pipeline is visible — rather than in storage, which
//! may not depend on `jarvis-tools`. The distinction matters because the two failures read differently on the
//! wire: a malformed identifier is a `422` about the caller's input, while a well-formed name for an
//! unregistered tool is a refusal the caller can only fix by installing something.
//!
//! # Why a created revision is a PROPOSAL, and it is not an oversight
//!
//! `SkillRevision::new` refuses a model-authored revision recorded `Active`, and a skill a **user** authors is
//! active from the outset — so this service makes the source `UserStatement` and the state `Active`, which is
//! the shape the type exists for. An agent-authored proposal is the other path, and it reaches the same store
//! through `propose`, which records `Proposed` and needs a promotion. **A client cannot choose the source
//! kind**, which is what keeps the two paths from being the same one with a flag: a caller that could declare
//! its own revision user-authored would be able to skip the promotion the whole design rests on.

use std::sync::Arc;

use jarvis_core::{
    CorrelationId, MemorySource, MemorySourceKind, Sensitivity, SkillId, SkillQuery, SkillRevision,
    SkillRevisionParts, SkillState, SkillStep, SystemClock, UtcTimestamp, WorkspaceId,
};
use jarvis_protocol::{
    CreateSkillRequest, DroppedFieldBody, ExportedSkill, ForgetSkillRequest, PromoteSkillRequest,
    SkillDeletionReceipt, SkillDetailReply, SkillExportReply, SkillListReply, SkillReference,
    SkillReply, SkillStepBody, SkillTransitionRequest,
};
use jarvis_storage::{
    DatabaseError, SqliteDatabase, StoredSkillRevision, delete_skill_revision,
    find_skill_revision_state, load_local_identity, read_workspace_skill_revision_states,
    record_skill_revision,
};

/// Whether a tool identifier names a tool this deployment can actually run.
///
/// A named alias rather than the bare trait object, because it appears in a struct field, a constructor
/// parameter, and two closures — and `clippy::type_complexity` is right that a spelled-out `Arc<dyn Fn(&str) ->
/// bool + Send + Sync>` in each place is harder to read than the fact it encodes: **membership**, not structure.
/// The `Send + Sync` is required because the daemon calls this from a spawned task.
pub type ToolMembership = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The most revisions one listing or export page returns.
///
/// A bound rather than the caller's value alone, because `limit` arrives from the wire and an unbounded one is
/// a read whose cost grows with the user's procedure library. Refused rather than clamped when a caller asks
/// for more, following the memory page: clamping would let a caller believe it asked for a larger page than it
/// received.
pub const MAX_SKILL_PAGE: u32 = 200;

/// Why a skill operation could not be performed.
///
/// Not `Eq` or `Clone`, because [`DatabaseError`] is neither — it wraps a `sqlx::Error`, which carries an
/// opaque source. A variant that exists to report an infrastructure failure has no use for structural
/// comparison, which is the same trade the memory service's error makes.
#[derive(Debug)]
pub enum SkillServiceError {
    /// The request named a value that is not a member of a closed set.
    UnknownValue {
        /// The field the value belonged to.
        field: &'static str,
        /// The value that was not recognized.
        value: String,
    },
    /// The request asked for more than [`MAX_SKILL_PAGE`].
    PageTooLarge {
        /// The requested size.
        requested: u32,
        /// The bound.
        maximum: u32,
    },
    /// A revision the caller named does not exist in this workspace.
    ///
    /// Reported as absent rather than as forbidden when it belongs to another workspace, because "not yours"
    /// confirms that something exists — the rule the memory and session surfaces follow.
    NotFound,
    /// The revision belongs to another workspace, which is the boundary no read may cross.
    ///
    /// Distinct from [`Self::NotFound`] in the **service** even though the route renders both the same way:
    /// the distinction is what a caller of this module needs, and collapsing them here would make the isolation
    /// rule unobservable to the one place that could test it.
    ForeignWorkspace,
    /// The caller's counter is stale, so the write was refused.
    Conflict,
    /// The domain refused the revision or the transition.
    Refused {
        /// The refusal, by its stable name.
        reason: &'static str,
        /// A human-readable explanation.
        detail: String,
    },
    /// The storage layer failed.
    ///
    /// The source is kept so a log line can explain what happened while the **client** gets one fixed message:
    /// a `DatabaseError`'s text can name a path, a table, or a SQL statement, and a skill operation's caller is
    /// not the audience for any of those.
    Storage(DatabaseError),
}

impl std::fmt::Display for SkillServiceError {
    /// Renders the failure for a **log**, which is the one place the storage source is read.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "storage: {error}"),
            other => formatter.write_str(&other.detail()),
        }
    }
}

impl SkillServiceError {
    /// Returns the detail a client may be shown, which never echoes stored content or a database message.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::UnknownValue { field, value } => {
                format!("{field} {value:?} is not a recognized value")
            }
            Self::PageTooLarge { requested, maximum } => {
                format!("a page of {requested} was requested; the maximum is {maximum}")
            }
            Self::NotFound => "no skill revision with that identifier in this workspace".to_owned(),
            // Deliberately the **same** message as `NotFound`: a caller must not be able to learn that another
            // workspace holds a revision with a given identifier, which is exactly what a distinct message
            // would disclose. The service keeps them apart; the wire does not.
            Self::ForeignWorkspace => {
                "no skill revision with that identifier in this workspace".to_owned()
            }
            Self::Conflict => {
                "the skill revision changed since it was read; re-read it and retry".to_owned()
            }
            Self::Refused { reason, detail } => format!("{reason}: {detail}"),
            Self::Storage(_) => "the local database is not available".to_owned(),
        }
    }
}

impl From<DatabaseError> for SkillServiceError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::SkillRevisionNotFound => Self::NotFound,
            DatabaseError::SkillConflict => Self::Conflict,
            // Both skill refusals keep their reason, because each one is actionable in a different way and the
            // message is the only thing the caller can act on: a promotion refused for a missing approver, a
            // self-approval, or a wrong state are three different fixes.
            DatabaseError::SkillPromotionRefused { reason } => Self::Refused {
                reason: "promotion",
                detail: reason.to_owned(),
            },
            DatabaseError::SkillTransitionRefused { reason } => Self::Refused {
                reason: "transition",
                detail: reason.to_owned(),
            },
            DatabaseError::StoredSkillInvalid { field } => Self::Refused {
                reason: "invalid_revision",
                detail: format!("the revision has an invalid {field}"),
            },
            other => Self::Storage(other),
        }
    }
}

/// The skill surface, over one database and the composed tool registry.
#[derive(Clone)]
pub struct SkillService {
    database: Arc<SqliteDatabase>,
    /// Whether a tool identifier names a tool this deployment can actually run.
    ///
    /// A predicate rather than the pipeline, because this service needs exactly one fact from it — membership —
    /// and holding the whole pipeline would let a later verb reach for something this surface should not do.
    /// `None` means no tool surface is composed, and then a structural check alone applies: with no registry
    /// there is nothing to check against, and refusing every step would conflate "this deployment has no tools"
    /// with "this step names a bad tool".
    known_tool: Option<ToolMembership>,
}

impl SkillService {
    /// Builds the service over the daemon's database and the composed tool pipeline.
    ///
    /// The pipeline is read **once** here to derive the membership predicate, so a tool registry that changes
    /// later cannot make this service and the executor disagree about which tools exist.
    #[must_use]
    pub fn new(
        database: Arc<SqliteDatabase>,
        tools: Option<&Arc<crate::tool_pipeline::ToolPipeline>>,
    ) -> Self {
        let known_tool = tools.map(|pipeline| {
            let pipeline = Arc::clone(pipeline);
            Arc::new(move |name: &str| {
                jarvis_tools::ToolId::new(name).is_ok_and(|id| pipeline.registry().get(&id).is_ok())
            }) as ToolMembership
        });
        Self {
            database,
            known_tool,
        }
    }

    /// Returns the workspace every operation is scoped to.
    ///
    /// Loaded from the seeded local identity rather than taken from a request. A failure is reported as
    /// storage, because an absent identity row means the database was not migrated rather than that the caller
    /// did something wrong.
    async fn workspace(&self) -> Result<WorkspaceId, SkillServiceError> {
        let identity = load_local_identity(&self.database).await?;
        identity
            .workspace_id()
            .parse()
            .map_err(|_| SkillServiceError::UnknownValue {
                field: "workspace_id",
                value: identity.workspace_id().to_owned(),
            })
    }

    /// The validator every storage call is given, which is the membership rule described in the module doc.
    ///
    /// Returned as a boxed predicate rather than a closure so it can be passed into the storage functions'
    /// borrowed `ToolValidator` — the identifier the storage crate checks is the same one creation validates
    /// against the registry, so the two cannot disagree about what a step may name.
    fn validator(&self) -> Box<dyn Fn(&str) -> bool + Send + Sync + '_> {
        match &self.known_tool {
            // Membership when a registry exists: a well-formed identifier naming an unregistered tool is
            // refused, which is the failure the caller can only fix by installing something.
            Some(known) => {
                let known = Arc::clone(known);
                Box::new(move |name: &str| known(name))
            }
            // Structure alone when none does. `jarvis_tools::ToolId`'s shape, restated here because this crate
            // cannot call it with no pipeline to compare against — and the storage crate checks the same shape
            // so the two agree about what a step may name.
            None => Box::new(structural_tool_identifier),
        }
    }

    /// Loads one revision, scoped to the workspace, with its counter.
    async fn load_scoped(
        &self,
        revision_id: &str,
        workspace: WorkspaceId,
    ) -> Result<StoredSkillRevision, SkillServiceError> {
        let validator = self.validator();
        let stored =
            find_skill_revision_state(&self.database, revision_id, validator.as_ref()).await?;
        // The workspace check is here rather than in the query, because a row in another workspace must be
        // reported as **absent** rather than as forbidden — the rule that keeps "not yours" from confirming
        // that something exists. The service distinguishes the two internally so the isolation rule is
        // testable; the route renders both the same way.
        if stored.revision().workspace_id() != workspace {
            return Err(SkillServiceError::ForeignWorkspace);
        }
        Ok(stored)
    }

    /// Lists the workspace's revisions, newest first, whatever their state.
    ///
    /// # Why this is not the *usable* read
    ///
    /// A listing answers "which procedures do I have, and which are usable" — so a proposal awaiting a
    /// promotion and an archived revision being kept for audit both belong here. The retrieval read
    /// (`read_usable_skill_revisions`) deliberately omits them, because something a caller cannot use is not a
    /// choice; an inspection surface is the one place the whole picture is the answer.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::PageTooLarge`] when the limit exceeds [`MAX_SKILL_PAGE`].
    pub async fn list(&self, limit: u32) -> Result<SkillListReply, SkillServiceError> {
        check_page(limit)?;
        let workspace = self.workspace().await?;
        let validator = self.validator();
        let stored = read_workspace_skill_revision_states(
            &self.database,
            workspace.to_string().as_str(),
            limit,
            validator.as_ref(),
        )
        .await?;
        let skills: Vec<SkillReference> = stored.iter().map(reference_of).collect();
        Ok(SkillListReply {
            returned: u32::try_from(skills.len()).unwrap_or(u32::MAX),
            skills,
            limit,
        })
    }

    /// Reads one revision, with the procedure's text.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::NotFound`] when the revision is not in this workspace.
    pub async fn read(&self, revision_id: &str) -> Result<SkillDetailReply, SkillServiceError> {
        let workspace = self.workspace().await?;
        let stored = self.load_scoped(revision_id, workspace).await?;
        Ok(detail_of(&stored, workspace))
    }

    /// Records a new revision, which is the creation surface `P4-012` recorded as missing.
    ///
    /// # Why this records an **active, user-authored** revision
    ///
    /// A skill reaches the store in one of two ways, and they are different on purpose. A **person** writing a
    /// procedure through this route is the user stating how something is done, so the source is `UserStatement`
    /// and the revision is usable immediately — nothing needs approving because no agent proposed it. An
    /// **agent** proposing one goes through [`Self::propose`], which records `Proposed` and requires a
    /// promotion that names its approver. **A client cannot choose which path it is on**, which is what keeps
    /// the promotion rule from being a flag a caller could clear.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::Refused`] when a step names a tool this deployment cannot run, when a
    /// correction names a predecessor belonging to another procedure, or when the domain refuses the revision —
    /// and [`SkillServiceError::NotFound`] when a correction names a predecessor that does not exist.
    pub async fn create(
        &self,
        request: &CreateSkillRequest,
    ) -> Result<SkillReply, SkillServiceError> {
        let workspace = self.workspace().await?;
        let at = UtcTimestamp::now(&SystemClock);

        // A correction's skill comes from the **predecessor**, read rather than trusted: a caller naming a
        // skill that does not own the revision it replaces would make the chain describe two procedures, which
        // is the rule the storage transition also enforces. Read here first so the failure is a named refusal
        // rather than a foreign-key silence.
        let (skill_id, supersedes) = if let Some(predecessor) = &request.supersedes {
            let stored = self.load_scoped(predecessor, workspace).await?;
            let stated = request
                .skill_id
                .as_ref()
                .ok_or(SkillServiceError::Refused {
                    reason: "missing_skill",
                    detail:
                        "a correction must name the skill it belongs to, because the revision it \
                             replaces is what determines that and the caller must confirm it"
                            .to_owned(),
                })?
                .parse::<SkillId>()
                .map_err(|_| SkillServiceError::UnknownValue {
                    field: "skill_id",
                    value: request.skill_id.clone().unwrap_or_default(),
                })?;
            if stated != stored.revision().skill_id() {
                return Err(SkillServiceError::Refused {
                    reason: "skill_mismatch",
                    detail: "the named skill is not the one the replaced revision belongs to"
                        .to_owned(),
                });
            }
            (stated, Some(stored.revision().revision_id()))
        } else {
            // A new procedure's skill is issued here. A caller-supplied identifier could collide with
            // something else, and the platform is the only party that can issue one.
            if request.skill_id.is_some() {
                return Err(SkillServiceError::Refused {
                    reason: "skill_id_not_allowed",
                    detail: "a new procedure's skill identity is issued by the platform; supply `skill_id` \
                             only when correcting an existing one"
                        .to_owned(),
                });
            }
            (SkillId::new(), None)
        };

        let steps = self.build_steps(&request.steps)?;
        let revision = SkillRevision::new(SkillRevisionParts {
            skill_id,
            workspace_id: workspace,
            revision_id: SkillId::new(),
            version: request.author_version.trim().to_owned(),
            description: request.description.clone(),
            steps,
            // The **person** is the author, which is what makes this revision usable without a promotion.
            source: MemorySource::of_kind(MemorySourceKind::UserStatement, actor_locator())
                .map_err(|error| SkillServiceError::Refused {
                    reason: "invalid_source",
                    detail: format!("{error}"),
                })?,
            // The most cautious classification that still permits use: a procedure's prose and its instructions
            // both reach a model, so a new one is `Internal` until its author says otherwise. A default of
            // `Public` would make an unclassified procedure disclosable to any destination, which is the
            // direction that fails open.
            sensitivity: Sensitivity::Internal,
            state: SkillState::Active,
            supersedes,
            dropped_fields: Vec::new(),
            run_id: None,
            created_by_actor_id: self.actor_id().await?,
            correlation_id: CorrelationId::new(),
            created_at: at,
        })
        .map_err(|error| SkillServiceError::Refused {
            reason: "invalid_revision",
            detail: format!("{error}"),
        })?;

        record_skill_revision(&self.database, &revision).await?;

        // A declared correction writes its second leg only after the successor is durable. A crash between them
        // leaves a successor that declares what it replaced and a predecessor that does not yet point forward,
        // which is the recoverable direction: the chain's second leg is derivable from the first.
        if let Some(predecessor) = supersedes {
            jarvis_storage::supersede_skill_revision(
                &self.database,
                &predecessor.to_string(),
                &revision.revision_id().to_string(),
            )
            .await?;
        }

        // Read back so the reply reports the counter the store actually holds — a creation that returned the
        // value its in-memory revision was built with would present `1` where the store may already be at `2`
        // if the supersession link was written.
        let stored = self
            .load_scoped(&revision.revision_id().to_string(), workspace)
            .await?;
        Ok(reply_of(
            &stored,
            if supersedes.is_some() {
                "corrected".to_owned()
            } else {
                "created".to_owned()
            },
            supersedes.map(|predecessor| format!("superseded {predecessor}")),
        ))
    }

    /// Promotes a proposal, which is the decision `ADR-0117` §4 makes the only route to `Active`.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::Conflict`] when the counter is stale, and [`SkillServiceError::Refused`]
    /// when the state, the approver, or the authorship refuses the promotion.
    pub async fn promote(
        &self,
        revision_id: &str,
        request: &PromoteSkillRequest,
    ) -> Result<SkillReply, SkillServiceError> {
        let workspace = self.workspace().await?;
        // Read first, so a foreign revision is reported as absent rather than promoted — the isolation rule
        // applies to a write exactly as it does to a read.
        self.load_scoped(revision_id, workspace).await?;
        let validator = self.validator();
        let promoted = jarvis_storage::promote_skill_revision(
            &self.database,
            revision_id,
            request.expected_version,
            &request.approver_actor_id,
            UtcTimestamp::now(&SystemClock),
            validator.as_ref(),
        )
        .await?;
        let stored = self
            .load_scoped(&promoted.revision_id().to_string(), workspace)
            .await?;
        Ok(reply_of(
            &stored,
            "promoted".to_owned(),
            Some(format!("approved by {}", request.approver_actor_id)),
        ))
    }

    /// Disables a revision by archiving it: retained for audit, not offered for use.
    ///
    /// # Why disable is `archive` rather than a new state
    ///
    /// A procedure an operator sets aside must not be offered to a model, and must remain readable — because
    /// "why is this not being used" is a question the inspection surface exists to answer, and a deleted row
    /// cannot answer it. That is exactly what `Archived` is, so adding a `Disabled` state would be a second
    /// vocabulary for one fact.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::Conflict`] on a stale counter and [`SkillServiceError::Refused`] when the
    /// revision is already archived.
    pub async fn disable(
        &self,
        revision_id: &str,
        request: &SkillTransitionRequest,
    ) -> Result<SkillReply, SkillServiceError> {
        let workspace = self.workspace().await?;
        self.load_scoped(revision_id, workspace).await?;
        let validator = self.validator();
        let archived = jarvis_storage::archive_skill_revision(
            &self.database,
            revision_id,
            request.expected_version,
            UtcTimestamp::now(&SystemClock),
            validator.as_ref(),
        )
        .await?;
        let stored = self
            .load_scoped(&archived.revision_id().to_string(), workspace)
            .await?;
        Ok(reply_of(
            &stored,
            "disabled".to_owned(),
            Some("set aside and no longer offered; retained for inspection".to_owned()),
        ))
    }

    /// Enables a disabled revision, returning it to the state its promotion record implies.
    ///
    /// # Why the restored state is derived rather than chosen
    ///
    /// A revision that was never promoted returns to `Proposed`, and one that was returns to `Active`. Asking
    /// the caller which it should be would let a disabled **proposal** be enabled straight to `Active`,
    /// bypassing the promotion that `ADR-0117` §4 requires — so the derivation is the domain's and this verb
    /// cannot express the bypass.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::Conflict`] on a stale counter and [`SkillServiceError::Refused`] when the
    /// revision is not archived.
    pub async fn enable(
        &self,
        revision_id: &str,
        request: &SkillTransitionRequest,
    ) -> Result<SkillReply, SkillServiceError> {
        let workspace = self.workspace().await?;
        self.load_scoped(revision_id, workspace).await?;
        let validator = self.validator();
        let restored = jarvis_storage::restore_skill_revision(
            &self.database,
            revision_id,
            request.expected_version,
            UtcTimestamp::now(&SystemClock),
            validator.as_ref(),
        )
        .await?;
        let stored = self
            .load_scoped(&restored.revision_id().to_string(), workspace)
            .await?;
        Ok(reply_of(
            &stored,
            "enabled".to_owned(),
            Some(format!(
                "returned to {} from its promotion record",
                stored.revision().state().as_str()
            )),
        ))
    }

    /// Deletes a revision, returning a receipt of what was removed.
    ///
    /// # Why the receipt is built before the delete
    ///
    /// Everything it reports is read from the row, so an after-the-fact count would always be zero — the rule
    /// the memory deletion follows, and the reason the counts are computed in this order rather than inside
    /// the storage function.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::Conflict`] on a stale counter, [`SkillServiceError::Refused`] when another
    /// revision declares a supersession with this one, and [`SkillServiceError::NotFound`] when it is absent.
    pub async fn forget(
        &self,
        revision_id: &str,
        request: &ForgetSkillRequest,
    ) -> Result<SkillDeletionReceipt, SkillServiceError> {
        let workspace = self.workspace().await?;
        let stored = self.load_scoped(revision_id, workspace).await?;
        let revision = stored.revision();

        let removed_description_chars =
            u32::try_from(revision.description().chars().count()).unwrap_or(u32::MAX);
        let removed_steps = u32::try_from(revision.steps().len()).unwrap_or(u32::MAX);
        let removed_dropped_fields =
            u32::try_from(revision.dropped_fields().len()).unwrap_or(u32::MAX);
        let cleared_supersession =
            revision.supersedes().is_some() || revision.superseded_by().is_some();

        delete_skill_revision(&self.database, revision_id, request.expected_version).await?;

        Ok(SkillDeletionReceipt {
            revision_id: revision_id.to_owned(),
            removed_description_chars,
            removed_steps,
            removed_dropped_fields,
            cleared_supersession,
            // Never empty, and the first entry is the reason the field exists: a procedure that was **offered as
            // context** reached a model, and a provider holds its own copy of any prompt under its own retention
            // policy. This platform cannot reach that copy, and the architecture document requires it be
            // surfaced rather than implied by a receipt that reads as total.
            unreachable: vec![
                "Models that received this procedure as context may retain their own copies of the prompt \
                 under their own retention policies."
                    .to_owned(),
                "Backups taken before this deletion may still contain the revision until they expire."
                    .to_owned(),
            ],
        })
    }

    /// Exports every revision the workspace holds, including proposals and archived ones.
    ///
    /// # Errors
    ///
    /// Returns [`SkillServiceError::PageTooLarge`] for an oversized page.
    pub async fn export(&self, limit: u32) -> Result<SkillExportReply, SkillServiceError> {
        check_page(limit)?;
        let workspace = self.workspace().await?;
        let validator = self.validator();
        let stored = read_workspace_skill_revision_states(
            &self.database,
            workspace.to_string().as_str(),
            limit,
            validator.as_ref(),
        )
        .await?;
        let skills: Vec<ExportedSkill> = stored.iter().map(|s| exported_of(s, workspace)).collect();
        Ok(SkillExportReply {
            workspace_id: workspace.to_string(),
            exported_at: UtcTimestamp::now(&SystemClock).to_string(),
            count: u32::try_from(skills.len()).unwrap_or(u32::MAX),
            skills,
            exclusions: vec![
                "A revision deleted with `forget` is removed outright and is **absent** here: a skill has no \
                 automatic ingest that could resurrect it, so no tombstone is written."
                    .to_owned(),
                "Superseded revisions are included, because the chain is what makes 'which procedure ran' \
                 answerable."
                    .to_owned(),
                "Models that received a procedure as context are outside this platform's reach."
                    .to_owned(),
            ],
        })
    }

    /// Builds validated steps from a creation request.
    ///
    /// Every step goes through [`SkillStep::new`], so the identifier rule, the version bound, and the
    /// instruction bound all apply — and the **membership** check is this service's addition, applied before
    /// construction so a well-formed name for an unregistered tool is refused with a reason the caller can act
    /// on rather than being stored as a procedure that cannot run.
    fn build_steps(
        &self,
        requested: &[jarvis_protocol::CreateSkillStep],
    ) -> Result<Vec<SkillStep>, SkillServiceError> {
        let mut steps = Vec::with_capacity(requested.len());
        for step in requested {
            if let Some(known) = &self.known_tool
                && !known(&step.tool)
            {
                return Err(SkillServiceError::Refused {
                    reason: "unknown_tool",
                    detail: format!(
                        "{} is not a tool this deployment can run, so the step could never execute",
                        step.tool
                    ),
                });
            }
            steps.push(
                SkillStep::new(
                    step.position,
                    step.tool.clone(),
                    step.tool_version.clone(),
                    step.instruction.clone(),
                    structural_tool_identifier,
                )
                .map_err(|error| SkillServiceError::Refused {
                    reason: "invalid_step",
                    detail: format!("{error}"),
                })?,
            );
        }
        Ok(steps)
    }

    /// Returns the actor a creation is attributed to.
    ///
    /// The seeded local user, read from the identity rather than supplied. This is the value the self-approval
    /// guard compares a promotion's approver against, which is why a client cannot name it — a caller able to
    /// choose its own author could choose one that differs from its approver and defeat the guard.
    async fn actor_id(&self) -> Result<String, SkillServiceError> {
        let identity = load_local_identity(&self.database).await?;
        Ok(identity.user_id().to_owned())
    }
}

/// The locator a service-authored source carries.
///
/// A fixed string rather than an identifier, because a source's locator names **where the claim came from** and
/// this one came from the person through the API. A per-request value here would be a field nothing reads, and
/// the correlation identifier already answers "which request".
fn actor_locator() -> &'static str {
    "api:skills"
}

/// Returns whether an identifier has the shape of a tool identifier.
///
/// `jarvis_tools::ToolId`'s rule — a dotted identifier with a non-empty namespace and name — restated here
/// because this crate reaches the registry through the pipeline rather than by constructing identifiers. The
/// storage crate checks the same shape, so the two agree about what a step may name; the **membership** rule is
/// the pipeline's and is applied in [`SkillService::build_steps`].
fn structural_tool_identifier(identifier: &str) -> bool {
    identifier
        .split_once('.')
        .is_some_and(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
}

/// Refuses a page larger than the bound.
fn check_page(limit: u32) -> Result<(), SkillServiceError> {
    if limit > MAX_SKILL_PAGE {
        return Err(SkillServiceError::PageTooLarge {
            requested: limit,
            maximum: MAX_SKILL_PAGE,
        });
    }
    Ok(())
}

/// Projects a stored revision onto its wire reference.
fn reference_of(stored: &StoredSkillRevision) -> SkillReference {
    let revision = stored.revision();
    SkillReference {
        revision_id: revision.revision_id().to_string(),
        skill_id: revision.skill_id().to_string(),
        author_version: revision.version().to_owned(),
        state: revision.state().as_str().to_owned(),
        sensitivity: revision.sensitivity().as_str().to_owned(),
        source_kind: revision.source().kind().as_str().to_owned(),
        source_locator: revision.source().locator().to_owned(),
        tool_ids: revision
            .steps()
            .iter()
            .map(|step| step.tool().to_owned())
            .collect(),
        created_at: revision.created_at().to_string(),
        updated_at: revision.updated_at().to_string(),
        promoted_by_actor_id: revision.promoted_by_actor_id().map(str::to_owned),
        promoted_at: revision.promoted_at().map(|at| at.to_string()),
        superseded_by: revision.superseded_by().map(|id| id.to_string()),
        supersedes: revision.supersedes().map(|id| id.to_string()),
        version_counter: stored.version_counter(),
    }
}

/// Projects a stored revision onto its detail reply, which is the one shape carrying the text.
///
/// `workspace` is passed so the usability verdict is the **domain's own** — [`ineligibility_for`] asks
/// `SkillQuery::is_eligible`, the same predicate retrieval asks. Computing it here from `state` and
/// `superseded_by` was the first version, and it **disagreed with retrieval**: the domain reports a revision
/// that is both archived and superseded as `not_usable` (its severity order puts the state before the chain),
/// while a hand-written check that looked at `superseded_by` first reported `superseded`. Two statements of one
/// rule is what the divergence needs, so there is now one.
fn detail_of(stored: &StoredSkillRevision, workspace: WorkspaceId) -> SkillDetailReply {
    let revision = stored.revision();
    let unusable = ineligibility_for(revision, workspace);
    SkillDetailReply {
        reference: reference_of(stored),
        description: revision.description().to_owned(),
        steps: revision
            .steps()
            .iter()
            .map(|step| SkillStepBody {
                position: step.position(),
                tool: step.tool().to_owned(),
                tool_version: step.tool_version().to_owned(),
                instruction: step.instruction().to_owned(),
            })
            .collect(),
        dropped_fields: revision
            .dropped_fields()
            .iter()
            .map(|dropped| DroppedFieldBody {
                field: dropped.name().to_owned(),
                reason: dropped.reason().as_str().to_owned(),
                authority_bearing: dropped.reason().is_authority_bearing(),
            })
            .collect(),
        is_usable: unusable.is_none(),
        unusable_reason: unusable,
    }
}

/// Projects a stored revision onto a control verb's reply.
fn reply_of(stored: &StoredSkillRevision, outcome: String, detail: Option<String>) -> SkillReply {
    SkillReply {
        reference: reference_of(stored),
        outcome,
        detail,
    }
}

/// Projects a stored revision onto an exported revision.
fn exported_of(stored: &StoredSkillRevision, workspace: WorkspaceId) -> ExportedSkill {
    let detail = detail_of(stored, workspace);
    ExportedSkill {
        reference: detail.reference,
        description: detail.description,
        steps: detail.steps,
        dropped_fields: detail.dropped_fields,
    }
}

/// Returns the reason a revision would not be offered, if it would not be.
///
/// Exposed so a caller that wants the retrieval verdict without a destination — an operator asking "would this
/// be used" — can ask the **same** question retrieval asks, with the destination substituted for the most
/// permissive one. Duplicating the check here would be a second statement of the eligibility rules.
#[must_use]
pub fn ineligibility_for(revision: &SkillRevision, workspace: WorkspaceId) -> Option<String> {
    let query = SkillQuery::new(workspace, Sensitivity::Restricted);
    query
        .is_eligible(revision)
        .err()
        .map(|reason| reason.code().to_owned())
}
