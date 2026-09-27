//! The memory surface: what JARVIS remembers, and the verbs that change it.
//!
//! `docs/api/contracts.md` sketches this as `get`/`search`/`correct`/`forget` plus a `DeletionReceipt`, and
//! `docs/architecture/memory-and-context.md` gives the rules the verbs have to satisfy. Before this slice
//! the memory domain, its storage, its candidate pipeline, its retrieval, and its embedding port all existed
//! and were tested, and **nothing could reach any of them**: no route, no CLI verb, no reader. This module is
//! the daemon half of the surface that changes that.
//!
//! # The scope rule, and why it is here rather than in each handler
//!
//! Every operation takes the workspace from a **loaded** `LocalIdentity`, never from a request. That is the
//! same rule the tool-call route follows for its workspace, and it is what makes "client A's memory cannot
//! enter client B's context" a transport property rather than a check each handler has to remember. This
//! deployment has one seeded local workspace, so today the rule cannot fail — and it is written this way so
//! that it still holds when `P4-009` adds sharing, which is exactly the shape `P4-002` recorded as the thing
//! to look for.
//!
//! # Why remembers go through the candidate pipeline
//!
//! A stored memory has a confidence cap, a sensitivity floor, a normalized claim, and a deduplication key,
//! and `P4-003`'s pipeline is what applies all four in one place. A remember handler that built a
//! `MemoryRecord` directly would re-derive them, which is how two paths come to disagree about what a
//! `UserStatement` at `Confirmed` means. So the handler builds a `MemoryCandidate`, runs `admit`, and stores
//! what the pipeline returned.
//!
//! The consequence is that a remember over the wire **can be refused**, and the refusal is reported rather
//! than worked around: a caller is told which rule refused it, because the caller is the party who can act
//! on it.

use std::sync::Arc;

use jarvis_core::{
    CandidateRefusal, ContextTrust, CorrelationId, EntityRef, MemoryAdmission, MemoryCandidate,
    MemoryConfidence, MemoryId, MemoryRecord, MemoryRecordParts, MemorySearchKey, MemorySource,
    MemorySourceKind, MemoryTrust, MemoryType, Sensitivity, StructuredClaim, SystemClock,
    UtcTimestamp, WorkspaceId,
};
use jarvis_protocol::{
    ClaimBody, CorrectMemoryRequest, DeletionReceipt, ExportedMemory, ForgetMemoryRequest,
    MemoryDetailReply, MemoryExportReply, MemoryListReply, MemoryReference, MemoryReply,
    MemorySearchHit, MemorySearchReply, MemorySearchRequest, RememberRequest, SignalContribution,
};
use jarvis_storage::{
    DatabaseError, SqliteDatabase, StoredMemory, apply_memory_transition,
    count_memory_entity_links, find_entity, find_memory_including_deleted, load_local_identity,
    purge_memory, read_all_memories, read_retrievable_memories, read_workspace_memories,
    record_memory,
};

/// The most claims one listing, search, or export page returns.
///
/// A bound rather than the caller's value alone, because `limit` arrives from the wire and an unbounded one
/// is a read whose cost grows with the user's history. Refused rather than clamped when a caller asks for
/// more, following the run-event page: clamping would let a caller believe it asked for a larger page than
/// it received.
pub const MAX_MEMORY_PAGE: u32 = 200;

/// The importance a remember gets when the caller states none.
///
/// The middle of the range rather than the top. A default that outranked explicit user statements would make
/// the ranking's importance signal meaningless, and the middle is the value that says "unremarkable" without
/// saying "unimportant".
pub const DEFAULT_MEMORY_IMPORTANCE: u8 = 2;

/// Why a memory operation could not be performed.
///
/// Not `Eq` or `Clone`, because [`DatabaseError`] is neither — it wraps a `sqlx::Error`, which carries an
/// opaque source. That is the right trade: a variant that exists to report an infrastructure failure has no
/// use for structural comparison, and making the wrapper comparable would mean discarding the source.
#[derive(Debug)]
pub enum MemoryServiceError {
    /// The request named a value that is not a member of a closed set.
    UnknownValue {
        /// The field the value belonged to.
        field: &'static str,
        /// The value that was not recognized.
        value: String,
    },
    /// The request asked for more than [`MAX_MEMORY_PAGE`].
    PageTooLarge {
        /// The requested size.
        requested: u32,
        /// The bound.
        maximum: u32,
    },
    /// A claim the caller named does not exist in this workspace.
    ///
    /// The identifier is deliberately **not** carried. Every consumer maps this to one fixed message and one
    /// status, and the identifier the caller already has is in its own request — a field nothing reads is the
    /// declared-but-unconstructed shape this repository removes wherever it finds it. It was removed here
    /// after the compiler reported it unread, which is exactly how that shape should be found.
    NotFound,
    /// The candidate pipeline refused the claim.
    Refused {
        /// The refusal, by its stable name.
        reason: &'static str,
        /// A human-readable explanation.
        detail: String,
    },
    /// The stored value does not agree with the request.
    Conflict,
    /// The storage layer failed.
    ///
    /// The source is kept so a log line can explain what happened while the **client** gets one fixed
    /// message: a `DatabaseError`'s text can name a path, a table, or a SQL statement, and a memory
    /// operation's caller is not the audience for any of those. The variant is matched without reading the
    /// field, which is why it is boxed-in by type rather than by a string — and why the `Display` impl below
    /// is the only thing that touches it.
    Storage(DatabaseError),
}

impl std::fmt::Display for MemoryServiceError {
    /// Renders the failure for a **log**, which is the one place the storage source is read.
    ///
    /// Separate from [`Self::detail`], which is what a client may see. Two renderings rather than one,
    /// because the same string reaching both destinations is how a database path ends up in an API response.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "storage: {error}"),
            other => formatter.write_str(&other.detail()),
        }
    }
}

impl MemoryServiceError {
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
            Self::NotFound => "no memory with that identifier in this workspace".to_owned(),
            Self::Refused { reason, detail } => format!("{reason}: {detail}"),
            Self::Conflict => {
                "the memory was changed by another writer; re-read it and retry".to_owned()
            }
            // Deliberately generic: a `DatabaseError`'s text can name a path or a SQL statement, so it is
            // never forwarded to a client.
            Self::Storage(_) => "the local database is not available".to_owned(),
        }
    }
}

impl From<DatabaseError> for MemoryServiceError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::MemoryNotFound => Self::NotFound,
            DatabaseError::MemoryConflict => Self::Conflict,
            // **A tombstone is a refusal, not an infrastructure failure**, and the first version of this
            // mapping got it wrong: it fell through to `Storage`, so a client that re-stated a claim the user
            // had deleted was told "the local database is not available". That sends an operator to check a
            // daemon that is running. The durable check lives in `record_memory` rather than in the candidate
            // pipeline's `tombstoned` stage, because only storage can answer the question atomically.
            DatabaseError::MemoryTombstoned => Self::Refused {
                reason: "tombstoned",
                detail: "the claim was deleted in this workspace and must not return".to_owned(),
            },
            other => Self::Storage(other),
        }
    }
}

/// The memory surface, over one database.
#[derive(Clone)]
pub struct MemoryService {
    database: Arc<SqliteDatabase>,
}

impl MemoryService {
    /// Builds the service over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// Returns the workspace every operation is scoped to.
    ///
    /// Loaded from the seeded local identity rather than taken from a request, which is the scope rule this
    /// module's doc explains. A failure is reported as storage, because an absent identity row means the
    /// database was not migrated rather than that the caller did something wrong.
    async fn workspace(&self) -> Result<WorkspaceId, MemoryServiceError> {
        let identity = load_local_identity(&self.database).await?;
        identity
            .workspace_id()
            .parse()
            .map_err(|_| MemoryServiceError::UnknownValue {
                field: "workspace_id",
                value: identity.workspace_id().to_owned(),
            })
    }

    /// Lists the workspace's claims, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::PageTooLarge`] when the limit exceeds [`MAX_MEMORY_PAGE`], and a
    /// storage failure when a row cannot be read.
    pub async fn list(&self, limit: u32) -> Result<MemoryListReply, MemoryServiceError> {
        check_page(limit)?;
        let workspace = self.workspace().await?;
        // The **retrieval** read, not the export read: a listing answers "what do you remember", and an
        // export answers "what do you hold". Including tombstones here would show a user claims they cannot
        // read, with no text and no way to act on them.
        let stored = read_workspace_memories(&self.database, workspace, limit).await?;
        let memories: Vec<MemoryReference> = stored.iter().map(reference_of).collect();
        Ok(MemoryListReply {
            returned: u32::try_from(memories.len()).unwrap_or(u32::MAX),
            memories,
            limit,
        })
    }

    /// Reads one claim, with its content.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::NotFound`] when the claim is not in this workspace. A claim in another
    /// workspace is reported as absent rather than as forbidden, matching the session rule: "not yours"
    /// confirms that something exists.
    pub async fn read(&self, memory_id: &str) -> Result<MemoryDetailReply, MemoryServiceError> {
        let workspace = self.workspace().await?;
        let stored = self.load_scoped(memory_id, workspace).await?;
        Ok(detail_of(&stored))
    }

    /// Searches the workspace's claims with the retrieval rules, and explains the ranking.
    ///
    /// # Errors
    ///
    /// Returns a refusal for an unrecognized enum value and a storage failure when the candidate read fails.
    pub async fn search(
        &self,
        request: &MemorySearchRequest,
    ) -> Result<MemorySearchReply, MemoryServiceError> {
        let limit = request.limit.unwrap_or(MAX_MEMORY_PAGE);
        check_page(limit)?;
        let workspace = self.workspace().await?;

        // The candidate window, then the pipeline's own eligibility and ranking. The read applies what SQL
        // can decide; `MemoryQuery::is_eligible` and `rank` apply the rest, so the search a user gets is the
        // same computation retrieval performs rather than a second implementation of it.
        let candidates = read_retrievable_memories(&self.database, workspace, limit).await?;
        let records: Vec<MemoryRecord> = candidates
            .into_iter()
            .map(StoredMemory::into_record)
            .collect();

        let query = Self::build_query(workspace, request)?;
        // No semantic index on this path: nothing writes an embedding yet, which is the limit `P4-005`,
        // `P4-006`, and `P4-007` each record. The context says so explicitly rather than leaving the caller
        // to infer it from a zero signal.
        let context = jarvis_core::ScoringContext::without_semantics();
        let considered = u32::try_from(records.len()).unwrap_or(u32::MAX);
        let selection = jarvis_core::rank(&records, &query, &context, &jarvis_core::SIGNAL_WEIGHTS);

        let matches: Vec<MemorySearchHit> = selection
            .scored()
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(hit_of)
            .collect();
        Ok(MemorySearchReply {
            matches,
            considered,
            limit,
        })
    }

    /// Stores a claim the caller states.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::Refused`] when the candidate pipeline refuses the claim, carrying the
    /// rule that refused it.
    pub async fn remember(
        &self,
        request: &RememberRequest,
    ) -> Result<MemoryReply, MemoryServiceError> {
        let workspace = self.workspace().await?;
        let memory_type = parse_memory_type(&request.memory_type)?;
        let source_kind = parse_source_kind(&request.source_kind)?;
        let entities = self
            .resolve_entities(workspace, &request.entity_ids)
            .await?;

        let candidate = MemoryCandidate {
            workspace_id: workspace,
            content: request.content.clone(),
            // The pipeline owns the trust and the sensitivity floors. The classification states the *type*
            // and the *source*, which is all it is for, and the proposed confidence is the ceiling the
            // source permits — a caller cannot raise it, because `admit` caps it again and a model inference
            // is refused above `Unverified` by `MemoryRecord::new` regardless.
            classification: jarvis_core::CandidateClassification {
                memory_type,
                source_kind,
            },
            proposed_sensitivity: Sensitivity::Public,
            proposed_confidence: confidence_ceiling(source_kind),
            importance: request.importance.unwrap_or(DEFAULT_MEMORY_IMPORTANCE),
            proposed_entities: entities.clone(),
            structured_claim: claim_from_body(request.claim.as_ref())?,
            source_locator: "api:memories".to_owned(),
            source_excerpt_hash: None,
            run_id: None,
            supersedes: request
                .supersedes
                .as_ref()
                .map(|value| parse_memory_id(value))
                .transpose()?,
            created_by_actor_id: "local-user".to_owned(),
            correlation_id: CorrelationId::new(),
        };

        // Supersession is declared, so the candidate names the claim it corrects and the pipeline links the
        // two. The `existing` context is deliberately **not** supplied: a remember is a *new* claim, and
        // handing the pipeline a same-key neighbour would turn a duplicate into a correction — the inference
        // `ADR-0045` refuses.
        // The tombstone is answered by `record_memory` rather than here, and the context says `false` because
        // this path has not read it — the storage check is the one that is atomic with the insert, so a
        // re-ingest of a deleted claim is refused by the layer that can see the tombstone row. Supplying a
        // guess would be worse than `false`: `true` would refuse a claim that is not deleted, and a value read
        // here would still be stale by the time the insert ran.
        let now = UtcTimestamp::now(&SystemClock);
        let context = jarvis_core::CandidateContext {
            resolved_entities: &entities,
            existing: None,
            superseded_by: None,
            tombstoned: false,
        };
        let admission =
            candidate
                .admit(&context)
                .map_err(|refusal| MemoryServiceError::Refused {
                    reason: refusal.as_str(),
                    detail: refusal_detail(&refusal),
                })?;
        let stored = self.store(workspace, &admission, now).await?;
        Ok(reply_of(
            &stored.memory,
            outcome_of(&admission, stored.wrote),
            None,
        ))
    }

    /// Replaces a claim's text with a corrected version.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::Conflict`] when the caller's version is stale, and
    /// [`MemoryServiceError::NotFound`] when the claim is not in this workspace.
    pub async fn correct(
        &self,
        memory_id: &str,
        request: &CorrectMemoryRequest,
    ) -> Result<MemoryReply, MemoryServiceError> {
        let workspace = self.workspace().await?;
        let existing = self.load_scoped(memory_id, workspace).await?;
        if existing.version() != request.expected_version {
            return Err(MemoryServiceError::Conflict);
        }

        // The replacement is built through the **domain** so every rule applies to it: bounded content, a
        // non-empty body, the source's confidence cap, and the sensitivity floor. Building a row by hand
        // would be the second path that disagrees about what a corrected claim may be.
        //
        // # Why the corrected claim does not inherit the original's validity window
        //
        // It cannot, and the domain is right to refuse it: a memory's `valid_from` may not precede its
        // `created_at`, because a window that opens before the record existed is a backdated claim and is how
        // a later correction would fail to outrank the thing it corrects. Carrying the original's `valid_from`
        // onto a row created now is exactly that, and the first version of this function did it — the
        // constructor refused with a message about a window the caller never supplied.
        //
        // So a correction's window opens when it is recorded. That **is** a real limit, and it is recorded
        // rather than papered over: correcting a claim that had not yet taken effect makes the correction
        // effective from now rather than from the original's start, so the pair reads as overlapping. A claim
        // whose window has **already** closed cannot be corrected at all — the correction would be born
        // expired, which the domain cannot express — and that is refused by name rather than stored.
        let at = UtcTimestamp::now(&SystemClock);
        if let Some(until) = existing.record().valid_until()
            && until.unix_nanos() <= at.unix_nanos()
        {
            return Err(MemoryServiceError::Refused {
                reason: "lapsed",
                detail: "the claim's validity window has already closed, so a correction of it would be \
                         born expired; record a new claim instead"
                    .to_owned(),
            });
        }
        let entities = match &request.entity_ids {
            Some(ids) => self.resolve_entities(workspace, ids).await?,
            None => existing.record().entities().to_vec(),
        };
        let replacement = MemoryRecord::new(MemoryRecordParts {
            id: MemoryId::new(),
            workspace_id: workspace,
            memory_type: existing.record().memory_type(),
            content: request.content.clone(),
            // The claim triple from the corrected version's text cannot be derived, so it is dropped rather
            // than carried over: keeping the old triple against new text would make the structured claim
            // describe something the content no longer says.
            structured_claim: None,
            source: MemorySource::of_kind(
                existing.record().source().kind(),
                existing.record().source().locator(),
            )
            .map_err(|_| MemoryServiceError::Refused {
                reason: "invalid_source",
                detail: "the corrected claim could not reuse the original's source".to_owned(),
            })?,
            confidence: existing.record().confidence(),
            importance: existing.record().importance(),
            sensitivity: existing.record().sensitivity(),
            entities,
            // The window opens now, because the domain refuses a `valid_from` before `created_at` — see the
            // note above. The bound is carried over unchanged: a correction does not extend a fact that was
            // meant to lapse, it only restates what the fact says.
            valid_from: Some(at),
            valid_until: existing.record().valid_until(),
            supersedes: Some(existing.record().id()),
            run_id: existing.record().run_id(),
            created_by_actor_id: existing.record().created_by_actor_id().to_owned(),
            correlation_id: existing.record().correlation_id(),
            created_at: at,
        })
        .map_err(|error| MemoryServiceError::Refused {
            reason: "invalid_replacement",
            detail: format!("{error}"),
        })?;

        let key = MemorySearchKey::new(
            replacement.memory_type(),
            replacement.entities(),
            replacement.content(),
        )
        .map_err(|error| MemoryServiceError::Refused {
            reason: "unkeyable",
            detail: format!("{error}"),
        })?;
        record_memory(&self.database, &replacement, &key).await?;

        // The original is archived **after** the replacement is durable. A crash between them leaves two
        // current claims rather than none, which is the recoverable direction: the user can see both and
        // correct again, whereas archiving first would leave no current claim for a correction that never
        // landed.
        apply_memory_transition(
            &self.database,
            memory_id,
            jarvis_storage::MemoryTransition::ReplaceWith(replacement.id()),
            UtcTimestamp::now(&SystemClock),
        )
        .await?;

        // Read back so the reply reports the version the store actually holds rather than the one the
        // in-memory value was built with. A correction immediately followed by another would otherwise
        // present an expectation derived from a value no write ever confirmed.
        let stored = self
            .load_scoped(&replacement.id().to_string(), workspace)
            .await?;
        Ok(reply_of(
            &stored,
            "corrected".to_owned(),
            Some(format!("superseded {memory_id}")),
        ))
    }
    /// Deletes a claim, returning a receipt of what was removed.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::Conflict`] when the caller's version is stale.
    pub async fn forget(
        &self,
        memory_id: &str,
        request: &ForgetMemoryRequest,
    ) -> Result<DeletionReceipt, MemoryServiceError> {
        let workspace = self.workspace().await?;
        let existing = self.load_scoped(memory_id, workspace).await?;
        if existing.version() != request.expected_version {
            return Err(MemoryServiceError::Conflict);
        }

        // Everything the receipt reports is read **before** the delete, because the cascade removes the rows
        // and an after-the-fact count would always be zero.
        let removed_content_chars =
            u32::try_from(existing.record().content().chars().count()).unwrap_or(u32::MAX);
        let removed_search_key = existing.search_key().is_some();
        let removed_entity_links = count_memory_entity_links(&self.database, memory_id).await?;
        let cleared_supersession =
            existing.record().supersedes().is_some() || existing.record().superseded_by().is_some();

        purge_memory(
            &self.database,
            memory_id,
            request.expected_version,
            request.allow_relearn,
            UtcTimestamp::now(&SystemClock),
        )
        .await?;

        Ok(DeletionReceipt {
            memory_id: memory_id.to_owned(),
            removed_content_chars,
            removed_search_key,
            tombstone_written: !request.allow_relearn,
            removed_entity_links,
            cleared_supersession,
            // Never empty, and the first entry is the reason the field exists: a provider that received this
            // claim as context holds its own copy, under its own retention policy, and this platform cannot
            // reach it. `docs/architecture/memory-and-context.md` requires that be "surfaced separately"
            // rather than implied by a receipt that reads as total.
            unreachable: vec![
                "Models and tools that received this claim as context may retain their own copies under \
                 their own retention policies."
                    .to_owned(),
                "Backups taken before this deletion may still contain the claim until they expire."
                    .to_owned(),
            ],
        })
    }

    /// Exports every claim the workspace holds, including archived and deleted ones.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryServiceError::PageTooLarge`] for an oversized page.
    pub async fn export(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<MemoryExportReply, MemoryServiceError> {
        check_page(limit)?;
        let workspace = self.workspace().await?;
        let stored = read_all_memories(&self.database, workspace, limit, offset).await?;
        let memories: Vec<ExportedMemory> = stored.iter().map(exported_of).collect();
        Ok(MemoryExportReply {
            workspace_id: workspace.to_string(),
            exported_at: UtcTimestamp::now(&SystemClock).to_string(),
            count: u32::try_from(memories.len()).unwrap_or(u32::MAX),
            memories,
            exclusions: vec![
                "A claim deleted with `forget` is removed outright and is **absent** here: only a tombstone \
                 survives, and it holds no content. A claim merely archived appears with its text."
                    .to_owned(),
                "Embeddings and derived indexes are not written yet, so there is nothing to export for them."
                    .to_owned(),
                "Provider-side copies of content sent as context are outside this platform's control."
                    .to_owned(),
            ],
        })
    }

    /// Loads one claim by identifier, refusing one outside the caller's workspace.
    ///
    /// The workspace check is a **comparison after the read** rather than a `WHERE` clause, because the two
    /// failures need different answers: "no such claim" and "not in your workspace" are the same thing to a
    /// caller that is not entitled to know the difference, and `find_memory` is also used by paths that
    /// legitimately hold an identifier without a workspace.
    async fn load_scoped(
        &self,
        memory_id: &str,
        workspace: WorkspaceId,
    ) -> Result<StoredMemory, MemoryServiceError> {
        let stored = find_memory_including_deleted(&self.database, memory_id).await?;
        if stored.record().workspace_id() != workspace {
            return Err(MemoryServiceError::NotFound);
        }
        Ok(stored)
    }

    /// Stores what the pipeline admitted, writing the entities a new claim names.
    async fn store(
        &self,
        workspace: WorkspaceId,
        admission: &MemoryAdmission,
        now: UtcTimestamp,
    ) -> Result<Stored, MemoryServiceError> {
        let Some(to_store) = admission.to_store() else {
            // A duplicate or an already-superseded claim writes nothing, and the caller is told which. The
            // existing memory is returned so the reply can name it, which makes "already remembered"
            // actionable rather than an error.
            let existing = match admission {
                MemoryAdmission::Duplicate { existing_memory_id } => existing_memory_id,
                MemoryAdmission::AlreadySuperseded { current_memory_id } => current_memory_id,
                // Unreachable: `to_store` returned `None`, so the variant is one of the two above. Spelled
                // out rather than wildcarded so a new no-write variant is a compile error here.
                MemoryAdmission::New(_)
                | MemoryAdmission::Proposal(_)
                | MemoryAdmission::Correction { .. } => {
                    return Err(MemoryServiceError::Refused {
                        reason: "no_write",
                        detail: "the claim was not stored".to_owned(),
                    });
                }
            };
            return Ok(Stored {
                memory: self.load_scoped(&existing.to_string(), workspace).await?,
                wrote: false,
            });
        };

        // **The entities are read, never written.** An earlier version of this loop called `record_entity`,
        // which invented any entity the caller named — so a request naming a subject the workspace did not
        // have was silently turned into a claim about a fabricated entity and answered `201`, and the identity
        // vocabulary became caller-controlled. `resolve_entities` now reads the store and refuses an unknown
        // or foreign subject, so by this point every entity exists in this workspace and the link below is a
        // foreign key onto a row that is really there.
        //
        // The link itself is written by `record_memory`, which inserts the `memory_entities` rows in the same
        // call as the memory row — so a claim cannot exist without the subjects it named.
        let record = MemoryRecord::new(MemoryRecordParts {
            id: MemoryId::new(),
            workspace_id: workspace,
            memory_type: to_store.memory_type,
            content: to_store.content.clone(),
            structured_claim: to_store.structured_claim.clone(),
            source: MemorySource::of_kind(to_store.source_kind, &to_store.source_locator).map_err(
                |_| MemoryServiceError::Refused {
                    reason: "invalid_source",
                    detail: "the claim's source was refused".to_owned(),
                },
            )?,
            confidence: to_store.confidence,
            importance: to_store.importance,
            sensitivity: to_store.sensitivity,
            entities: to_store.entities.clone(),
            valid_from: Some(now),
            valid_until: None,
            // The declared correction, read from the admission rather than from `MemoryToStore` — the
            // pipeline puts it on the `Correction` variant, which is what makes the link impossible to
            // omit at the point the pipeline decided there is one.
            supersedes: declared_supersession(admission),
            run_id: to_store.run_id,
            created_by_actor_id: to_store.created_by_actor_id.clone(),
            correlation_id: to_store.correlation_id,
            created_at: now,
        })
        .map_err(|error| MemoryServiceError::Refused {
            reason: "invalid_claim",
            detail: format!("{error}"),
        })?;

        let key = MemorySearchKey::new(record.memory_type(), record.entities(), record.content())
            .map_err(|error| MemoryServiceError::Refused {
            reason: "unkeyable",
            detail: format!("{error}"),
        })?;

        let (id, wrote) = match record_memory(&self.database, &record, &key).await {
            Ok(id) => (id, true),
            // **A duplicate is an outcome, not a failure**, and this is the branch that handles it. It is
            // reached on every re-statement, not only on a race, because `remember` supplies no `existing`
            // memory to the pipeline — so the pipeline decides `New` and the unique index is what catches the
            // duplicate. The first version of this function let the error escape, and a user restating a claim
            // they had already made was told "the memory was changed by another writer"; the gateway test that
            // asserted a repeated claim returns `200` is what found it.
            //
            // The existing memory is returned rather than a second row being written, which is what
            // "deduplicate / compare existing" requires. It is **not** reinforced here: reinforcement counts a
            // retrieval, and this request did not cause one.
            //
            // `wrote` is `false`, and that is the fact the reply needs: the pipeline's name for this admission
            // is `New`, which would report "remembered" for a request that stored nothing. So the outcome is
            // derived from *what happened* rather than from what the pipeline decided, and a caller can tell a
            // creation from a re-statement by the outcome as well as by the status.
            Err(DatabaseError::MemoryDuplicate { existing_memory_id }) => {
                (existing_memory_id, false)
            }
            Err(other) => return Err(other.into()),
        };
        Ok(Stored {
            memory: self.load_scoped(&id, workspace).await?,
            wrote,
        })
    }

    /// Builds the retrieval query a search runs, converting every wire value or refusing it.
    fn build_query(
        workspace: WorkspaceId,
        request: &MemorySearchRequest,
    ) -> Result<jarvis_core::MemoryQuery, MemoryServiceError> {
        let mut query = jarvis_core::MemoryQuery::new(workspace, UtcTimestamp::now(&SystemClock));
        match &request.text {
            // Present-and-empty is refused rather than treated as absent: a caller that sent an empty string
            // asked for something, and widening the result silently is the wrong reading of it.
            Some(text) if text.trim().is_empty() => {
                return Err(MemoryServiceError::UnknownValue {
                    field: "text",
                    value: String::new(),
                });
            }
            Some(text) => query = query.with_text(text.clone()),
            None => {}
        }
        if !request.memory_types.is_empty() {
            let types = request
                .memory_types
                .iter()
                .map(|value| parse_memory_type(value))
                .collect::<Result<Vec<MemoryType>, _>>()?;
            query = query.with_allowed_types(types);
        }
        if !request.entity_ids.is_empty() {
            let entities = request
                .entity_ids
                .iter()
                .map(|value| parse_entity_id(value))
                .collect::<Result<Vec<_>, _>>()?;
            query = query.with_entities(entities);
        }
        if let Some(value) = &request.minimum_trust {
            query = query.with_minimum_trust(parse_trust(value)?);
        }
        if let Some(value) = &request.minimum_confidence {
            query = query.with_minimum_confidence(parse_confidence(value)?);
        }
        // The destination is the **local** daemon, so the ceiling is `Restricted`: this is the machine the
        // memory is already on. A search through a remote model would take that model's placement as the
        // ceiling, which is what the run path does; a client listing its own memory is not a disclosure.
        Ok(query.with_destination(Sensitivity::Restricted))
    }

    /// Resolves the entity identifiers a request names, refusing an unparseable one.
    ///
    /// # Why an empty list is refused rather than filled in
    ///
    /// A memory must name at least one entity — the domain refuses one that does not, and the entity is what
    /// makes "what do I know about this person" an index seek rather than a text search. The obvious
    /// convenience is to attach a placeholder subject, and it is wrong for the reason `P4-001` records about
    /// entity resolution: a claim attached to a placeholder *looks* resolved, so a later question about the
    /// person it is really about will not find it, and nothing in the store says why. Refusing with the field
    /// named is a refusal the caller can act on, and inventing `MemoryCandidate::proposed_entities` from the
    /// text would be entity extraction, which no slice has built.
    ///
    /// # Why an unknown entity is refused rather than created
    ///
    /// The first version of this function only parsed, and the link step then **invented** any entity the
    /// caller named. Two consequences, both bad:
    ///
    /// - A request naming an entity the workspace does not have was silently turned into a claim about a
    ///   fabricated subject and answered `201`. Nothing distinguished it from a claim about a real person, so a
    ///   typo in an identifier produced a memory nobody could find or correct — the exact failure mode the
    ///   placeholder refusal above exists to prevent, left in place one layer down.
    /// - The identity vocabulary became caller-controlled. `docs/architecture/identity-and-workspaces.md`
    ///   requires that an entity be established through resolution — verified provider IDs, exact identifiers,
    ///   user confirmation, or a probabilistic match recorded as such — and a caller asserting one over the
    ///   wire is none of those.
    ///
    /// So the entity must already exist **in this workspace**, and the check is a read of the store rather than
    /// a trust of the request. `entity_ids` then means what its name says: the subjects the claim is about.
    async fn resolve_entities(
        &self,
        workspace: WorkspaceId,
        entity_ids: &[String],
    ) -> Result<Vec<EntityRef>, MemoryServiceError> {
        if entity_ids.is_empty() {
            return Err(MemoryServiceError::UnknownValue {
                field: "entity_ids",
                value: "(empty)".to_owned(),
            });
        }
        let mut resolved = Vec::with_capacity(entity_ids.len());
        for value in entity_ids {
            let entity_id = parse_entity_id(value)?;
            let stored = find_entity(&self.database, &entity_id.to_string())
                .await
                .map_err(|error| match error {
                    // A missing entity is a **refusal about the request**, not a storage failure: the caller
                    // named something the workspace does not have, and that is a value it can fix.
                    DatabaseError::EntityNotFound => MemoryServiceError::UnknownValue {
                        field: "entity_ids",
                        value: value.clone(),
                    },
                    other => other.into(),
                })?;
            // The entity must belong to **this** workspace, or a caller could link its claim to another
            // workspace's subject — a cross-workspace write dressed as a link.
            if stored.workspace_id() != workspace {
                return Err(MemoryServiceError::UnknownValue {
                    field: "entity_ids",
                    value: value.clone(),
                });
            }
            // A merged entity's claims belong to the winner and a deleted one's to nobody, so a new claim
            // against either would attach itself to a name that no longer denotes anything. `is_usable` is the
            // domain's own predicate rather than a comparison restated here.
            if !stored.is_usable() {
                return Err(MemoryServiceError::Refused {
                    reason: "entity_not_usable",
                    detail:
                        "an entity that is merged or deleted cannot be the subject of a new claim"
                            .to_owned(),
                });
            }
            resolved.push(EntityRef::confirmed(entity_id));
        }
        Ok(resolved)
    }
}

/// Returns the claim an admission declared it supersedes, if it declared one.
///
/// Read from the admission's `Correction` variant rather than from the `MemoryToStore` beside it, because the
/// two are not the same fact: `to_store` says *what to write* and the variant says *why*, and only the
/// variant knows a correction was declared. A caller reading the store struct would have to re-derive the
/// declaration, which is the second path `ADR-0045` exists to prevent.
fn declared_supersession(admission: &MemoryAdmission) -> Option<MemoryId> {
    match admission {
        MemoryAdmission::Correction { supersedes, .. } => Some(*supersedes),
        MemoryAdmission::New(_)
        | MemoryAdmission::Proposal(_)
        | MemoryAdmission::Duplicate { .. }
        | MemoryAdmission::AlreadySuperseded { .. } => None,
    }
}

/// Refuses a page larger than the bound.
fn check_page(limit: u32) -> Result<(), MemoryServiceError> {
    if limit == 0 || limit > MAX_MEMORY_PAGE {
        return Err(MemoryServiceError::PageTooLarge {
            requested: limit,
            maximum: MAX_MEMORY_PAGE,
        });
    }
    Ok(())
}

/// What a store operation produced: the claim as the store holds it, and whether anything was written.
///
/// # Why the two travel together
///
/// The pipeline's admission name and the fact of a write are different answers, and only the first was
/// available to the reply. `MemoryAdmission::New` is what the pipeline decides when it is given no `existing`
/// memory to compare against — which is every remember — so a duplicate is detected by the unique index
/// **after** that decision. A reply built from the admission alone therefore reports "remembered" for a
/// request that stored nothing, and the caller's status code and outcome would both be wrong in the same
/// direction. Pairing them makes the divergence unrepresentable rather than merely documented.
struct Stored {
    /// The claim as the store holds it.
    memory: StoredMemory,
    /// Whether this operation wrote a row.
    wrote: bool,
}

/// Returns the stable name of what an admission did, correcting for whether it actually wrote.
fn outcome_of(admission: &MemoryAdmission, wrote: bool) -> String {
    // A duplicate reaches here as `New` when the index caught it, so the write is the authority: nothing
    // stored means nothing was remembered, whatever the pipeline's own comparison decided.
    if !wrote {
        // The two no-write admissions that are **not** duplicates keep their own names, so a caller is told
        // which of the two happened rather than a single vague "already known".
        return match admission {
            MemoryAdmission::Duplicate { .. } | MemoryAdmission::New(_) => {
                "already_remembered".to_owned()
            }
            MemoryAdmission::AlreadySuperseded { .. } => "already_superseded".to_owned(),
            MemoryAdmission::Proposal(_) | MemoryAdmission::Correction { .. } => {
                // Unreachable: both carry a `to_store`, so a store of either writes. Spelled out rather than
                // wildcarded so a new no-write variant is a compile error here.
                "no_write".to_owned()
            }
        };
    }
    match admission {
        MemoryAdmission::New(_) => "remembered",
        MemoryAdmission::Proposal(_) => "proposed",
        MemoryAdmission::Correction { .. } => "corrected",
        MemoryAdmission::Duplicate { .. } => "already_remembered",
        MemoryAdmission::AlreadySuperseded { .. } => "already_superseded",
    }
    .to_owned()
}

/// Builds the reply for a stored claim.
fn reply_of(stored: &StoredMemory, outcome: String, detail: Option<String>) -> MemoryReply {
    let record = stored.record();
    MemoryReply {
        memory_id: record.id().to_string(),
        memory_type: record.memory_type().as_str().to_owned(),
        status: record.status().as_str().to_owned(),
        effective_status: record
            .effective_status_at(UtcTimestamp::now(&SystemClock))
            .as_str()
            .to_owned(),
        outcome,
        // The version *after* the operation, so a caller can immediately correct or delete what it just
        // wrote without a second read.
        version: stored.version(),
        detail,
    }
}

/// Builds the reference fields shared by listings, searches, and details.
fn reference_of(stored: &StoredMemory) -> MemoryReference {
    let record = stored.record();
    MemoryReference {
        memory_id: record.id().to_string(),
        memory_type: record.memory_type().as_str().to_owned(),
        source_kind: record.source().kind().as_str().to_owned(),
        status: record.status().as_str().to_owned(),
        effective_status: record
            .effective_status_at(UtcTimestamp::now(&SystemClock))
            .as_str()
            .to_owned(),
        importance: record.importance(),
        confidence: record.confidence().as_str().to_owned(),
        sensitivity: record.sensitivity().as_str().to_owned(),
        created_at: record.created_at().to_string(),
        updated_at: record.updated_at().to_string(),
        last_accessed_at: record.last_accessed_at().map(|at| at.to_string()),
        retrieval_count: record.retrieval_count(),
        // The value a correction or deletion must present. It is on the reference rather than only on a
        // write's reply, because the caller obtains an expectation by *reading* and a reply that omitted it
        // would leave the client re-reading and hoping — the lost update the guard exists to prevent.
        version: stored.version(),
        superseded_by: record.superseded_by().map(|id| id.to_string()),
    }
}

/// Builds the detail reply, which is the one place content is returned.
fn detail_of(stored: &StoredMemory) -> MemoryDetailReply {
    let record = stored.record();
    MemoryDetailReply {
        reference: reference_of(stored),
        content: record.content().to_owned(),
        claim: record.structured_claim().map(|claim| ClaimBody {
            subject: claim.subject().to_owned(),
            predicate: claim.predicate().to_owned(),
            object: claim.object().to_owned(),
        }),
        source_locator: record.source().locator().to_owned(),
        entity_ids: record
            .entities()
            .iter()
            .map(|entity| entity.entity_id().to_string())
            .collect(),
        // Both predicates are asked of the **domain**, so a client cannot apply one of the two conditions and
        // forget the other: a superseded claim and an unconfirmed one are both "not established".
        is_stated_as_fact: record.is_stateable_as_fact_at(UtcTimestamp::now(&SystemClock)),
        carries_untrusted_trust: record.context_trust() == ContextTrust::Untrusted,
    }
}

/// Builds one ranked search hit, with the arithmetic that produced its position.
fn hit_of(scored: &jarvis_core::ScoredMemory) -> MemorySearchHit {
    let contributions: Vec<SignalContribution> = scored
        .signals()
        .contributions(&jarvis_core::SIGNAL_WEIGHTS)
        .iter()
        // Zero contributions are omitted: a signal that produced nothing is not part of the explanation, and
        // a row of zeros makes the two or three that mattered harder to find.
        .filter(|(_, contribution)| *contribution > 0)
        .map(|(signal, contribution)| SignalContribution {
            signal: (*signal).to_owned(),
            contribution: *contribution,
        })
        .collect();
    MemorySearchHit {
        reference: MemoryReference {
            memory_id: scored.record().id().to_string(),
            memory_type: scored.record().memory_type().as_str().to_owned(),
            source_kind: scored.record().source().kind().as_str().to_owned(),
            status: scored.record().status().as_str().to_owned(),
            effective_status: scored
                .record()
                .effective_status_at(UtcTimestamp::now(&SystemClock))
                .as_str()
                .to_owned(),
            importance: scored.record().importance(),
            confidence: scored.record().confidence().as_str().to_owned(),
            sensitivity: scored.record().sensitivity().as_str().to_owned(),
            created_at: scored.record().created_at().to_string(),
            updated_at: scored.record().updated_at().to_string(),
            last_accessed_at: scored.record().last_accessed_at().map(|at| at.to_string()),
            retrieval_count: scored.record().retrieval_count(),
            // `0`, and this is the one place a reference carries no real version. The ranking holds a
            // `MemoryRecord`, which does not carry the optimistic-concurrency version, so a value here could
            // only be fabricated — and a fabricated expectation is worse than an absent one, because a
            // caller would present it and have a valid write refused. A caller that wants to change a search
            // result reads it with `memory show` first, which does carry the version. `0` can never match a
            // stored version, since versions start at one, so a mistake is a refusal rather than a write.
            version: 0,
            superseded_by: scored.record().superseded_by().map(|id| id.to_string()),
        },
        score: scored.total(),
        reason: scored.reason().as_str().to_owned(),
        is_a_match: scored.reason().is_a_match(),
        contributions,
    }
}

/// Builds one exported claim.
fn exported_of(stored: &StoredMemory) -> ExportedMemory {
    let record = stored.record();
    ExportedMemory {
        memory_id: record.id().to_string(),
        memory_type: record.memory_type().as_str().to_owned(),
        content: record.content().to_owned(),
        source_kind: record.source().kind().as_str().to_owned(),
        source_locator: record.source().locator().to_owned(),
        status: record.status().as_str().to_owned(),
        effective_status: record
            .effective_status_at(UtcTimestamp::now(&SystemClock))
            .as_str()
            .to_owned(),
        confidence: record.confidence().as_str().to_owned(),
        sensitivity: record.sensitivity().as_str().to_owned(),
        importance: record.importance(),
        claim: record.structured_claim().map(|claim| ClaimBody {
            subject: claim.subject().to_owned(),
            predicate: claim.predicate().to_owned(),
            object: claim.object().to_owned(),
        }),
        entity_ids: record
            .entities()
            .iter()
            .map(|entity| entity.entity_id().to_string())
            .collect(),
        created_at: record.created_at().to_string(),
        updated_at: record.updated_at().to_string(),
    }
}

/// Converts a claim body into the domain's normalized form.
fn claim_from_body(
    body: Option<&ClaimBody>,
) -> Result<Option<StructuredClaim>, MemoryServiceError> {
    let Some(body) = body else {
        return Ok(None);
    };
    StructuredClaim::new(&body.subject, &body.predicate, &body.object)
        .map(Some)
        .map_err(|error| MemoryServiceError::Refused {
            reason: "invalid_claim",
            detail: format!("{error}"),
        })
}

/// Parses a memory type name, or refuses it.
fn parse_memory_type(value: &str) -> Result<MemoryType, MemoryServiceError> {
    value.parse().map_err(|_| MemoryServiceError::UnknownValue {
        field: "memory_type",
        value: value.to_owned(),
    })
}

/// Parses a source kind name, or refuses it.
fn parse_source_kind(value: &str) -> Result<MemorySourceKind, MemoryServiceError> {
    value.parse().map_err(|_| MemoryServiceError::UnknownValue {
        field: "source_kind",
        value: value.to_owned(),
    })
}

/// Parses a trust name, or refuses it.
fn parse_trust(value: &str) -> Result<MemoryTrust, MemoryServiceError> {
    match value {
        "untrusted" => Ok(MemoryTrust::Untrusted),
        "derived" => Ok(MemoryTrust::Derived),
        "authoritative" => Ok(MemoryTrust::Authoritative),
        other => Err(MemoryServiceError::UnknownValue {
            field: "minimum_trust",
            value: other.to_owned(),
        }),
    }
}

/// Parses a confidence name, or refuses it.
fn parse_confidence(value: &str) -> Result<MemoryConfidence, MemoryServiceError> {
    value.parse().map_err(|_| MemoryServiceError::UnknownValue {
        field: "minimum_confidence",
        value: value.to_owned(),
    })
}

/// Parses a memory identifier, or refuses it.
fn parse_memory_id(value: &str) -> Result<MemoryId, MemoryServiceError> {
    value.parse().map_err(|_| MemoryServiceError::UnknownValue {
        field: "memory_id",
        value: value.to_owned(),
    })
}

/// Parses an entity identifier, or refuses it.
fn parse_entity_id(value: &str) -> Result<jarvis_core::EntityId, MemoryServiceError> {
    value.parse().map_err(|_| MemoryServiceError::UnknownValue {
        field: "entity_id",
        value: value.to_owned(),
    })
}

/// Returns the confidence ceiling a source kind permits.
///
/// A free function rather than a trait impl, because the mapping belongs to the pipeline — `admit` recomputes
/// it through `CandidateClassification::confidence_ceiling` — and this is only the value the request starts
/// at. Two copies of a cap is how the two come to disagree, so this delegates to the pipeline's own function
/// rather than restating the table.
const fn confidence_ceiling(kind: MemorySourceKind) -> MemoryConfidence {
    jarvis_core::CandidateClassification {
        memory_type: MemoryType::Semantic,
        source_kind: kind,
    }
    .confidence_ceiling()
}

/// Returns a short explanation for a refusal, without echoing the claim's content.
///
/// The refusal's own `Display` is used where it exists, because the domain's messages name the rule. Content
/// is never included: a refusal travels to a client and into a log, and a memory's text is user content.
fn refusal_detail(refusal: &CandidateRefusal) -> String {
    match refusal {
        CandidateRefusal::Content => "the claim is empty, whitespace, or too long".to_owned(),
        CandidateRefusal::Unkeyable => "the claim yields no usable search key".to_owned(),
        CandidateRefusal::Unsupported => "the claim has no usable source".to_owned(),
        CandidateRefusal::TooManyEntities => "the claim names too many entities".to_owned(),
        CandidateRefusal::EntityUnresolved => {
            "the claim names an entity this workspace does not have".to_owned()
        }
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owns a temporary profile directory and removes it on drop.
    struct TempProfile(std::path::PathBuf);

    impl TempProfile {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("jarvis-memory-{}", jarvis_core::scratch_tag()));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create temp profile: {error}"));
            Self(path)
        }

        fn database_path(&self) -> std::path::PathBuf {
            self.0.join(jarvis_storage::DEFAULT_DATABASE_FILENAME)
        }
    }

    impl Drop for TempProfile {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    /// Builds the service over a real migrated database, so the identity rows migration `0005` seeds are
    /// present rather than written by a fixture. The service reads its workspace from those rows, so a
    /// hand-written identity would be a fixture asserting its own premise.
    async fn service() -> (MemoryService, Arc<SqliteDatabase>, TempProfile) {
        let profile = TempProfile::new();
        let database = jarvis_storage::SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        let database = Arc::new(database);
        (MemoryService::new(Arc::clone(&database)), database, profile)
    }

    /// Records an entity, because `memory_entities` is a foreign key and a claim naming an unrecorded
    /// entity is refused at the link step.
    ///
    /// Written through the storage function rather than by the service, because the service deliberately has
    /// no entity-creation verb: naming an entity identifier is the caller's job and this fixture is the
    /// caller.
    async fn entity(database: &Arc<SqliteDatabase>) -> jarvis_core::EntityId {
        let id = jarvis_core::EntityId::new();
        jarvis_storage::record_entity(
            database,
            &jarvis_storage::NewEntity {
                id,
                workspace_id: must_parse(jarvis_storage::LOCAL_WORKSPACE_ID),
                kind: jarvis_storage::EntityKind::Person,
                label: "Fixture subject".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Confirmed,
                created_at: UtcTimestamp::now(&SystemClock),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("record entity: {error}"));
        id
    }

    fn must_parse(value: &str) -> WorkspaceId {
        value
            .parse()
            .unwrap_or_else(|_| panic!("the seeded workspace identifier must parse: {value}"))
    }

    /// Builds a remember request for a preference about one entity.
    fn remember_request(entity: &jarvis_core::EntityId, content: &str) -> RememberRequest {
        remember_request_str(&entity.to_string(), content)
    }

    /// Builds a remember request naming an entity by **text**.
    ///
    /// Separate from the typed helper because the refusals under test are about identifiers that do not denote
    /// anything: an `EntityId` that parses and exists would skip exactly the check being asserted.
    fn remember_request_str(entity_id: &str, content: &str) -> RememberRequest {
        RememberRequest {
            content: content.to_owned(),
            memory_type: "preference".to_owned(),
            source_kind: "user_statement".to_owned(),
            importance: None,
            entity_ids: vec![entity_id.to_owned()],
            claim: None,
            supersedes: None,
        }
    }

    /// **The whole lifecycle, end to end.** Remember, list, read, correct, and forget, with each step's
    /// effect asserted on the *next* read rather than on its own reply.
    ///
    /// Asserting on the reply alone would pass with a service that returned the right shape and wrote
    /// nothing. The correction is the step that makes this non-trivial: the replacement is a second row and
    /// the original is archived, so a listing that showed one claim and a read that returned the new text
    /// are two independent facts that both have to hold.
    #[tokio::test]
    async fn a_claim_can_be_remembered_read_corrected_and_forgotten() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;

        let remembered = service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));
        assert_eq!(remembered.outcome, "remembered");
        assert_eq!(remembered.version, 1, "a new claim's first version is one");
        let original = remembered.memory_id.clone();

        let listed = service
            .list(MAX_MEMORY_PAGE)
            .await
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(listed.returned, 1);
        assert_eq!(listed.memories[0].memory_id, original);

        let read = service
            .read(&original)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(read.content, "Prefers dark roast coffee");
        assert!(
            !read.carries_untrusted_trust,
            "the user's own statement is trusted input, so it is not fenced as external content"
        );
        assert!(
            read.is_stated_as_fact,
            "a confirmed user statement is stateable as a fact"
        );

        let corrected = service
            .correct(
                &original,
                &CorrectMemoryRequest {
                    content: "Prefers light roast coffee".to_owned(),
                    expected_version: 1,
                    entity_ids: None,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("correct: {error}"));
        assert_eq!(corrected.outcome, "corrected");
        assert_ne!(
            corrected.memory_id, original,
            "a correction is its own claim with a supersedes link, never an in-place overwrite"
        );

        // The original survives as a superseded reference rather than disappearing: `docs/api/contracts.md`
        // requires the obsolete claim remain readable, which is what makes the correction auditable.
        let superseded = service
            .read(&original)
            .await
            .unwrap_or_else(|error| panic!("read the superseded original: {error}"));
        assert_eq!(superseded.reference.status, "archived");
        assert_eq!(
            superseded.reference.superseded_by.as_deref(),
            Some(corrected.memory_id.as_str())
        );
        assert!(
            !superseded.is_stated_as_fact,
            "a superseded claim must not be offered as established, whatever its confidence"
        );
        assert_eq!(
            superseded.content, "Prefers dark roast coffee",
            "the superseded text is retained, because the correction is an audit trail"
        );

        let receipt = service
            .forget(
                &corrected.memory_id,
                &ForgetMemoryRequest {
                    expected_version: corrected.version,
                    allow_relearn: false,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("forget: {error}"));
        assert_eq!(receipt.removed_content_chars, 26);
        assert!(
            receipt.removed_search_key,
            "the key holds the words, so it goes with them"
        );
        assert!(receipt.tombstone_written);
        assert_eq!(receipt.removed_entity_links, 1);
        assert!(
            !receipt.unreachable.is_empty(),
            "a deletion receipt must never read as total: provider copies are outside this platform"
        );
    }

    /// **The version guard, in both directions.** A stale version is refused and a current one is
    /// accepted, so the test fails if the check is removed *or* if it refuses everything.
    ///
    /// The accepted case is what makes this a guard rather than a wall, and it is the direction a test that
    /// only asserted the refusal would miss.
    #[tokio::test]
    async fn a_write_against_a_stale_version_is_refused_and_a_current_one_is_not() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        let remembered = service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));

        let stale = service
            .correct(
                &remembered.memory_id,
                &CorrectMemoryRequest {
                    content: "Prefers light roast coffee".to_owned(),
                    expected_version: remembered.version + 1,
                    entity_ids: None,
                },
            )
            .await;
        assert!(
            matches!(stale, Err(MemoryServiceError::Conflict)),
            "a version the caller did not read must be refused, got {stale:?}"
        );

        let accepted = service
            .correct(
                &remembered.memory_id,
                &CorrectMemoryRequest {
                    content: "Prefers light roast coffee".to_owned(),
                    expected_version: remembered.version,
                    entity_ids: None,
                },
            )
            .await;
        assert!(
            accepted.is_ok(),
            "the version the caller was told is the version that must be accepted: {accepted:?}"
        );
    }

    /// **A claim deleted with a tombstone cannot be learned again, and one deleted with `allow_relearn`
    /// can.** Both directions, because "refused everything" would satisfy the first alone.
    ///
    /// This is the falsification test for the deletion guard: with the tombstone write removed from
    /// `purge_memory`, the second remember succeeds and this test fails.
    #[tokio::test]
    async fn a_forgotten_claim_does_not_return_unless_relearning_was_allowed() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        let text = "Prefers dark roast coffee";

        let remembered = service
            .remember(&remember_request(&subject, text))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));
        service
            .forget(
                &remembered.memory_id,
                &ForgetMemoryRequest {
                    expected_version: remembered.version,
                    allow_relearn: false,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("forget: {error}"));

        // The same words, at the same key, through the same pipeline. The tombstone is the only thing
        // standing between this and a resurrected claim.
        let refused = service.remember(&remember_request(&subject, text)).await;
        assert!(
            matches!(
                refused,
                Err(MemoryServiceError::Refused {
                    reason: "tombstoned",
                    ..
                })
            ),
            "a deleted claim must not be re-learned, got {refused:?}"
        );

        // And the same claim, deleted with the explicit undo, must come back — otherwise the flag is a
        // no-op that the test above cannot distinguish from a working tombstone.
        let replacement = "Prefers very dark roast coffee";
        let second = service
            .remember(&remember_request(&subject, replacement))
            .await
            .unwrap_or_else(|error| panic!("remember the replacement: {error}"));
        service
            .forget(
                &second.memory_id,
                &ForgetMemoryRequest {
                    expected_version: second.version,
                    allow_relearn: true,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("forget for relearning: {error}"));
        let relearned = service
            .remember(&remember_request(&subject, replacement))
            .await;
        assert!(
            relearned.is_ok(),
            "an explicit allow-relearn must remove the tombstone it undoes: {relearned:?}"
        );
    }

    /// **The scope guard's falsification test.** A claim the caller's workspace does not hold is reported as
    /// *absent*, never as forbidden.
    ///
    /// This is the test that fails if [`MemoryService::load_scoped`]'s comparison is replaced with
    /// `Ok(stored)`. The check is driven directly rather than through a second workspace's row, because
    /// producing one would mean either a fixture reaching past the storage API or a production verb whose
    /// only caller is a test — and the property to falsify is the comparison itself, which needs a
    /// *different* workspace identifier and not a second stored claim.
    ///
    /// The positive direction is asserted too: the claim must be found under the workspace that really holds
    /// it, or "refuses everything" would pass the negative half alone.
    #[tokio::test]
    async fn a_claim_is_only_readable_under_the_workspace_that_holds_it() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        let remembered = service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));

        // The claim is really stored, so the refusal below is the workspace comparison and not a missing row.
        let holder = must_parse(jarvis_storage::LOCAL_WORKSPACE_ID);
        assert!(
            service
                .load_scoped(&remembered.memory_id, holder)
                .await
                .is_ok(),
            "the claim must load under the workspace that holds it"
        );

        let elsewhere = jarvis_core::WorkspaceId::new();
        let refused = service.load_scoped(&remembered.memory_id, elsewhere).await;
        assert!(
            matches!(refused, Err(MemoryServiceError::NotFound)),
            "a claim outside the caller's workspace must read as absent, not as forbidden: {refused:?}"
        );

        // And the wire path reports the same thing, so the guard is not bypassed by `read` or `forget`
        // passing a workspace of their own choosing.
        let absent = service
            .read(&jarvis_core::MemoryId::new().to_string())
            .await;
        assert!(
            matches!(absent, Err(MemoryServiceError::NotFound)),
            "an identifier nothing holds must read as absent: {absent:?}"
        );
    }

    /// **A remember with no entity is refused rather than filed against a placeholder.**
    ///
    /// The refusal direction is the one that matters: a placeholder subject would make the claim *look*
    /// resolved, so a later question about the real subject would not find it and nothing in the store would
    /// say why. Asserting the refusal is what keeps the placeholder from being reintroduced as a convenience.
    #[tokio::test]
    async fn a_remember_that_names_no_entity_is_refused() {
        let (service, _database, _profile) = service().await;
        let mut request =
            remember_request(&jarvis_core::EntityId::new(), "Prefers dark roast coffee");
        request.entity_ids.clear();

        let refused = service.remember(&request).await;
        match refused {
            Err(MemoryServiceError::UnknownValue { field, .. }) => {
                assert_eq!(
                    field, "entity_ids",
                    "the refusal must name the field the caller can fix"
                );
            }
            other => panic!("an entity-less remember must be refused, got {other:?}"),
        }
    }

    /// **A search hit carries no usable version, and a read does.** The limit stated where it bites.
    ///
    /// `hit_of` reports `0` because the ranking does not hold the stored version, so a hit cannot be
    /// corrected without a `show` first. Asserting it keeps the limit visible: a future change that made the
    /// ranking carry a version would fail this test rather than silently making the CLI's flow optional.
    #[tokio::test]
    async fn a_search_hit_carries_no_version_and_a_read_does() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));

        let found = service
            .search(&MemorySearchRequest {
                text: Some("dark roast coffee".to_owned()),
                memory_types: Vec::new(),
                entity_ids: Vec::new(),
                minimum_trust: None,
                minimum_confidence: None,
                limit: Some(10),
            })
            .await
            .unwrap_or_else(|error| panic!("search: {error}"));
        assert_eq!(
            found.matches.len(),
            1,
            "the keyword signal must find the claim"
        );
        assert!(
            found.matches[0].is_a_match,
            "a keyword hit is a match, not a recency inclusion"
        );
        assert!(
            !found.matches[0].contributions.is_empty(),
            "the hit must explain its own score from stored components"
        );
        assert_eq!(
            found.matches[0].reference.version, 0,
            "a search hit must not fabricate a version the ranking never read"
        );

        let read = service
            .read(&found.matches[0].reference.memory_id)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(
            read.reference.version, 1,
            "the read is where a caller obtains the version a write must present"
        );

        // A hit's fabricated-looking `0` must be refused by the guard rather than accepted, because versions
        // start at one — that is what makes carrying `0` safe as an "I do not know" marker.
        let refused = service
            .forget(
                &found.matches[0].reference.memory_id,
                &ForgetMemoryRequest {
                    expected_version: found.matches[0].reference.version,
                    allow_relearn: false,
                },
            )
            .await;
        assert!(
            matches!(refused, Err(MemoryServiceError::Conflict)),
            "a version no row can hold must never be accepted as an expectation: {refused:?}"
        );
    }

    /// **The export is a full user read, and it says what it leaves out.**
    ///
    /// Both halves are asserted together because the exclusions are the reason the shape exists: an export
    /// that listed every row and claimed completeness would be the wrong artifact for a portability request.
    ///
    /// The name still says "deleted" even though `forget` purges rather than tombstones, because the property
    /// under test is that the export reflects what the workspace **holds** — and the assertion below is the
    /// one that found the exclusion text describing a path this verb does not take.
    #[tokio::test]
    async fn an_export_reports_what_the_workspace_holds_and_states_its_exclusions() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        let kept = service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember the kept claim: {error}"));
        let removed = service
            .remember(&remember_request(&subject, "Prefers light roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember the removed claim: {error}"));
        service
            .forget(
                &removed.memory_id,
                &ForgetMemoryRequest {
                    expected_version: removed.version,
                    allow_relearn: false,
                },
            )
            .await
            .unwrap_or_else(|error| panic!("forget: {error}"));

        let export = service
            .export(MAX_MEMORY_PAGE, 0)
            .await
            .unwrap_or_else(|error| panic!("export: {error}"));
        assert_eq!(
            export.count, 1,
            "a purged claim is gone from the export: `forget` removes the row and leaves only a content-free \
             tombstone, which is not an exported memory"
        );
        assert!(
            export
                .memories
                .iter()
                .any(|memory| memory.memory_id == kept.memory_id
                    && memory.content == "Prefers dark roast coffee"),
            "the live claim must be present with its text"
        );
        // **A recorded claim in the exclusion list was wrong, and this assertion is what found it.** The text
        // said "deleted claims appear as an empty record", which describes the `Delete` **transition** — the
        // form that clears text and keeps the row. It does not describe `forget`, which purges. The export's
        // own doc used the same wrong sentence, so the two agreed while both were describing a path no
        // deletion verb takes. Asserting the absence is what keeps the two from re-agreeing on the wrong thing.
        assert!(
            !export
                .memories
                .iter()
                .any(|memory| memory.memory_id == removed.memory_id),
            "a purged claim is absent from the export, not present as a tombstone"
        );
        assert!(
            !export.exclusions.is_empty(),
            "an export that looks complete and is not is worse than one that says what it left out"
        );

        let listed = service
            .list(MAX_MEMORY_PAGE)
            .await
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(
            listed.returned, 1,
            "the listing answers what is remembered, so a purged claim is absent from it too"
        );
    }

    /// A page larger than the bound is refused rather than clamped, so a caller cannot believe it asked
    /// for more than it received.
    #[tokio::test]
    async fn an_oversized_page_is_refused() {
        let (service, _database, _profile) = service().await;
        let refused = service.list(MAX_MEMORY_PAGE + 1).await;
        match refused {
            Err(MemoryServiceError::PageTooLarge { requested, maximum }) => {
                assert_eq!(requested, MAX_MEMORY_PAGE + 1);
                assert_eq!(maximum, MAX_MEMORY_PAGE);
            }
            other => panic!("an oversized page must be refused, got {other:?}"),
        }
    }

    /// **A remember naming an entity the workspace does not have is refused, not invented.**
    ///
    /// This is the `P4-010` finding, asserted at the **service** level because the service is where the defect
    /// lived: the link step called `record_entity`, which inserted any entity the caller named — so a request
    /// naming a subject that did not exist produced a claim about a fabricated one and answered `remembered`.
    ///
    /// The wording is deliberately different from any stored claim's, so a refusal cannot come from the
    /// duplicate index instead of the entity check.
    #[tokio::test]
    async fn a_remember_naming_an_unknown_entity_is_refused() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;
        service
            .remember(&remember_request(&subject, "Prefers dark roast coffee"))
            .await
            .unwrap_or_else(|error| panic!("remember: {error}"));

        let unknown = jarvis_core::EntityId::new().to_string();
        let refused = service
            .remember(&remember_request_str(
                &unknown,
                "a claim about a subject that was never established",
            ))
            .await;
        match refused {
            Err(MemoryServiceError::UnknownValue { field, value }) => {
                assert_eq!(field, "entity_ids");
                assert_eq!(
                    value, unknown,
                    "the refusal must name the identifier the caller supplied, so it can be fixed"
                );
            }
            other => panic!("an unknown entity must be refused by field, got {other:?}"),
        }

        // The entity was **not** created on the way to refusing. A check that stopped at the refusal would pass
        // against a service that invented the subject and then refused for another reason, so the store is asked
        // directly.
        let created = jarvis_storage::find_entity(&database, &unknown).await;
        assert!(
            matches!(created, Err(jarvis_storage::DatabaseError::EntityNotFound)),
            "the refused request must not have invented its entity, got {created:?}"
        );
    }

    /// **A remember naming another workspace's entity is refused**, or a caller could file a claim that points
    /// across a boundary every read respects.
    ///
    /// Asserted at the service level as well as in the acceptance gate, because this is the one write path where
    /// a caller-supplied identifier reaches a foreign key.
    #[tokio::test]
    async fn a_remember_naming_another_workspaces_entity_is_refused() {
        let (service, database, _profile) = service().await;
        let subject = entity(&database).await;

        // A second workspace with its own entity, recorded through the product's own functions.
        let foreign_workspace = jarvis_core::WorkspaceId::new();
        jarvis_storage::record_workspace(
            &database,
            &jarvis_storage::NewWorkspace {
                id: foreign_workspace,
                name: "Foreign".to_owned(),
                mode: jarvis_storage::WorkspaceMode::Local,
                data_policy: jarvis_storage::DataPolicy::Standard,
                created_at: UtcTimestamp::now(&SystemClock),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("record the foreign workspace: {error}"));
        let foreign_entity = jarvis_core::EntityId::new();
        jarvis_storage::record_entity(
            &database,
            &jarvis_storage::NewEntity {
                id: foreign_entity,
                workspace_id: foreign_workspace,
                kind: jarvis_storage::EntityKind::Person,
                label: "Foreign subject".to_owned(),
                attributes: None,
                confidence: MemoryConfidence::Confirmed,
                created_at: UtcTimestamp::now(&SystemClock),
            },
        )
        .await
        .unwrap_or_else(|error| panic!("record the foreign entity: {error}"));

        // The local subject is accepted, so the refusal below is the scope check and not a broken fixture.
        assert!(
            service
                .remember(&remember_request(&subject, "Prefers dark roast coffee"))
                .await
                .is_ok(),
            "the local subject must be accepted, or the refusal below proves nothing about scoping"
        );
        let refused = service
            .remember(&remember_request_str(
                &foreign_entity.to_string(),
                "a claim filed across the workspace boundary",
            ))
            .await;
        assert!(
            matches!(
                refused,
                Err(MemoryServiceError::UnknownValue {
                    field: "entity_ids",
                    ..
                })
            ),
            "another workspace's entity must be refused as an unknown value, got {refused:?}"
        );
    }
}
