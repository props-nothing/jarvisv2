//! The entity surface: what a memory is *about* (`P4-016`).
//!
//! `docs/architecture/memory-and-context.md` requires that every claim name a subject, and `P4-008` records the
//! consequence of there being no way to name one: "**No entity-creation surface exists**, so a remember is still
//! unreachable by a user of the shipped product." `P4-014` and `P4-015` record the same limit from their own
//! directions — a model's proposal and a session's summary each need an entity identifier nobody could obtain.
//! This module is the surface that removes it.
//!
//! # The rule this module keeps, and the one it must not break
//!
//! It keeps the architecture's: every memory names its subject, so a caller must be able to create and find
//! one. It must **not** break the rule `resolve_alias` documents — "**Ambiguous aliases remain separate
//! candidates**" — because a resolution answering with one entity would turn a guess into an identity, and
//! every later claim would inherit it. So a lookup returns **every** match, with the aliases that produced each
//! and whether they are verified.
//!
//! # Why a match carries a count of linked claims
//!
//! An operator merging duplicates has to know which side is the used one: a merge moves no links, so merging
//! the wrong direction leaves every claim attached to an entity whose status is `merged`. The count is the
//! evidence for that decision, and it is what makes exposing `merge` defensible rather than an unrecoverable
//! operation behind one keystroke.

use std::sync::Arc;

use jarvis_core::{
    EntityId, EntityMatch, MemoryConfidence, MemorySourceKind, SystemClock, UtcTimestamp,
    WorkspaceId,
};
use jarvis_protocol::{
    AddAliasRequest, CreateEntityRequest, EntityAliasReply, EntityDetailReply, EntityListReply,
    EntityLookupReply, EntityMatchReply, EntityReply, MergeEntityRequest,
};
use jarvis_storage::{
    DatabaseError, NewEntity, SqliteDatabase, StoredAlias, StoredEntity, find_entity,
    load_local_identity, merge_entities, read_alias_candidates, read_entities_by_label,
    read_entity_aliases, read_workspace_entities, record_alias, record_entity,
};

use crate::memory_service::MAX_MEMORY_PAGE;

/// The most aliases one detail reply carries.
///
/// A bound rather than the caller's value: an alias list is unbounded in principle, since a connector adds one
/// provider identifier per observed account, and a detail read returning all of them is a read whose cost grows
/// with how much has been observed about one name.
pub const MAX_ALIASES: u32 = 64;

/// The most entities one lookup returns.
///
/// Separate from [`MAX_MEMORY_PAGE`] because the two answer different questions and the smaller bound is the
/// honest one here: a name matching two hundred entities is a name that needs disambiguation, and returning two
/// hundred rows of them does not help the caller decide. A refusal at the bound would be worse — a duplicated
/// label is a real state, and the candidates are the answer.
pub const MAX_ENTITY_MATCHES: u32 = 50;

/// Why an entity operation could not be performed.
///
/// Distinct from [`crate::memory_service::MemoryServiceError`] even though the two are nearly the same, because
/// the refusals are not: this surface's caller errors are a merge's direction and an alias's contradiction, and
/// reporting either as a *memory* problem would send an operator to the wrong surface.
#[derive(Debug)]
pub enum EntityServiceError {
    /// The request named a value that is not a member of a closed set.
    UnknownValue {
        /// The field the value belonged to.
        field: &'static str,
        /// The value that was not recognized.
        value: String,
    },
    /// The request asked for more entities than a lookup returns.
    PageTooLarge {
        /// The requested size.
        requested: u32,
        /// The bound.
        maximum: u32,
    },
    /// The entity the caller named does not exist.
    NotFound,
    /// The operation is refused by a rule the caller can act on.
    Refused {
        /// The refusal, by a stable name rather than a sentence, so a caller can branch on it.
        reason: &'static str,
        /// A human-readable explanation that names the remedy.
        detail: String,
    },
    /// The storage layer failed.
    Storage(DatabaseError),
}

impl std::fmt::Display for EntityServiceError {
    /// Renders the failure for a **log**, which is the only place the storage source is read.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "storage: {error}"),
            Self::UnknownValue { field, .. } => write!(formatter, "unknown value for {field}"),
            Self::PageTooLarge { requested, maximum } => {
                write!(formatter, "page {requested} exceeds {maximum}")
            }
            Self::NotFound => formatter.write_str("no such entity"),
            Self::Refused { reason, .. } => write!(formatter, "refused: {reason}"),
        }
    }
}

impl EntityServiceError {
    /// Returns the message a client may see.
    ///
    /// Separate from the `Display` impl above, because the same string reaching both a log and an API response
    /// is how a table name or a filesystem path ends up in a reply.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::UnknownValue { field, value } => format!("{value} is not a recognized {field}"),
            Self::PageTooLarge { requested, maximum } => {
                format!("{requested} exceeds the maximum of {maximum}")
            }
            Self::NotFound => "no entity exists for the requested identifier".to_owned(),
            Self::Refused { detail, .. } => detail.clone(),
            Self::Storage(_) => "the entity store is unavailable".to_owned(),
        }
    }

    /// Returns the stable name of the refusal, for a caller that branches on it.
    #[must_use]
    pub const fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Refused { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

impl From<DatabaseError> for EntityServiceError {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::EntityNotFound => Self::NotFound,
            // A variant naming a **request field** is the caller's, so it becomes a refusal rather than a `503`;
            // the rest are infrastructure. The same split the memory surface makes.
            DatabaseError::InvalidMemoryRequest { field } => Self::Refused {
                reason: "invalid_entity",
                detail: format!("the entity {field} is invalid"),
            },
            // A `merged` entity is a name that no longer denotes anything, so addressing one is the caller's
            // error rather than a missing row — and saying which it is matters, because "not found" would send
            // an operator looking for a row that is right there.
            DatabaseError::StoredMemoryInvalid { .. } => Self::Storage(error),
            other => Self::Storage(other),
        }
    }
}

/// The entity surface over the daemon's database.
#[derive(Clone)]
pub struct EntityService {
    database: Arc<SqliteDatabase>,
}

impl EntityService {
    /// Builds the service over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// Returns the workspace every operation is scoped to.
    ///
    /// Read from the seeded identity rather than taken from a request, which is this module's scope rule and
    /// the same one the memory surface follows: it is what makes "client A's entity cannot be the subject of
    /// client B's claim" a property of the transport rather than a check in each handler.
    ///
    /// # Errors
    ///
    /// Returns a storage failure when the identity row cannot be read, which means the database was not
    /// migrated rather than that the caller did something wrong.
    pub async fn workspace(&self) -> Result<WorkspaceId, EntityServiceError> {
        let identity = load_local_identity(&self.database).await?;
        identity
            .workspace_id()
            .parse()
            .map_err(|_| EntityServiceError::UnknownValue {
                field: "workspace_id",
                value: identity.workspace_id().to_owned(),
            })
    }

    /// Creates an entity, returning it with its (empty) alias list.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::Refused`] for an unknown kind, an unknown confidence, a blank or oversized
    /// label, or attributes that are not a JSON document.
    pub async fn create(
        &self,
        request: &CreateEntityRequest,
    ) -> Result<EntityDetailReply, EntityServiceError> {
        let workspace = self.workspace().await?;
        let kind = parse_kind(&request.kind)?;
        let confidence = parse_confidence(&request.confidence)?;

        // Checked here as well as by the schema so a rejection names the field rather than surfacing as a
        // constraint failure naming a table.
        let label = request.label.trim();
        if label.is_empty() || label.chars().count() > 256 {
            return Err(EntityServiceError::Refused {
                reason: "invalid_entity",
                detail: "the entity label must be 1 to 256 characters".to_owned(),
            });
        }
        if let Some(attributes) = request.attributes.as_deref()
            && !is_plausible_document(attributes)
        {
            return Err(EntityServiceError::Refused {
                reason: "invalid_entity",
                detail: "the entity attributes must be a JSON object or array of 2 to 8192 bytes"
                    .to_owned(),
            });
        }

        let id = EntityId::new();
        record_entity(
            &self.database,
            &NewEntity {
                id,
                workspace_id: workspace,
                kind,
                label: label.to_owned(),
                attributes: request.attributes.clone(),
                confidence,
                created_at: UtcTimestamp::now(&SystemClock),
            },
        )
        .await?;

        // Read back through the decoder rather than building the reply from the request, so a column the writer
        // set differently from the caller's expectation is caught here — the reason every reply in the memory
        // surface is built from a stored row too.
        let stored = find_entity(&self.database, &id.to_string()).await?;
        Ok(EntityDetailReply {
            entity: self.reply_of(&stored).await?,
            aliases: Vec::new(),
        })
    }

    /// Reads one entity with its aliases.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::NotFound`] when the entity does not exist.
    pub async fn read(&self, entity_id: &str) -> Result<EntityDetailReply, EntityServiceError> {
        let stored = find_entity(&self.database, entity_id).await?;
        let aliases = read_entity_aliases(&self.database, stored.id(), MAX_ALIASES)
            .await?
            .iter()
            .map(alias_reply)
            .collect();
        Ok(EntityDetailReply {
            entity: self.reply_of(&stored).await?,
            aliases,
        })
    }

    /// Lists the workspace's usable entities, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::PageTooLarge`] for an out-of-range page.
    pub async fn list(&self, limit: u32) -> Result<EntityListReply, EntityServiceError> {
        check_page(limit)?;
        let workspace = self.workspace().await?;
        let stored = read_workspace_entities(&self.database, workspace, limit).await?;
        let mut entities = Vec::with_capacity(stored.len());
        for entity in &stored {
            entities.push(self.reply_of(entity).await?);
        }
        Ok(EntityListReply {
            returned: u32::try_from(entities.len()).unwrap_or(u32::MAX),
            entities,
            limit,
        })
    }

    /// Looks an entity up by its **own label**, returning every match.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::Refused`] for a blank label and a storage failure otherwise.
    pub async fn lookup_by_label(
        &self,
        label: &str,
        limit: u32,
    ) -> Result<EntityLookupReply, EntityServiceError> {
        let label = label.trim();
        if label.is_empty() {
            return Err(EntityServiceError::Refused {
                reason: "invalid_entity",
                detail: "a label lookup requires a non-blank label".to_owned(),
            });
        }
        let workspace = self.workspace().await?;
        // Clamped rather than refused, which is the opposite of what a page bound does elsewhere: a duplicated
        // label is a **real state**, so the matches are the answer, and refusing would deny a caller the
        // disambiguation it asked for. The clamp keeps the read bounded without making "this name is ambiguous"
        // an error.
        let limit = limit.clamp(1, MAX_ENTITY_MATCHES);
        let stored = read_entities_by_label(&self.database, workspace, label, limit).await?;
        let mut matches = Vec::with_capacity(stored.len());
        for entity in &stored {
            matches.push(EntityMatchReply {
                entity: self.reply_of(entity).await?,
                matched_aliases: Vec::new(),
                // A label match is the entity's own name, which is not an identity claim about an alias, so it
                // is not "verified" in that sense. A caller needing that asks by alias.
                verified: false,
            });
        }
        Ok(EntityLookupReply {
            returned: u32::try_from(matches.len()).unwrap_or(u32::MAX),
            matches,
        })
    }

    /// Resolves an alias, returning every candidate and whether each is verified.
    ///
    /// # Why this reads candidates rather than `resolve_alias`
    ///
    /// `resolve_alias` answers "is there a **verified** identity for this name", which is the right answer for a
    /// caller that must act on an identity and the wrong one for a lookup: it discards the probabilistic
    /// candidates, so a caller would see "no entity" for a name several entities are guessed from. This reads
    /// both and reports the verification per match, which is "remain separate candidates" applied to the wire.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::Refused`] for a blank kind or value.
    pub async fn lookup_by_alias(
        &self,
        alias_kind: &str,
        alias_value: &str,
        limit: u32,
    ) -> Result<EntityLookupReply, EntityServiceError> {
        let alias_kind = alias_kind.trim();
        let alias_value = alias_value.trim();
        if alias_kind.is_empty() || alias_value.is_empty() {
            return Err(EntityServiceError::Refused {
                reason: "invalid_alias",
                detail: "an alias lookup requires both a kind and a value".to_owned(),
            });
        }
        let workspace = self.workspace().await?;
        let limit = limit.clamp(1, MAX_ENTITY_MATCHES);
        let candidates =
            read_alias_candidates(&self.database, workspace, alias_kind, alias_value).await?;

        let mut matches: Vec<EntityMatchReply> = Vec::new();
        for alias in candidates
            .iter()
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
        {
            let stored = match find_entity(&self.database, &alias.entity_id().to_string()).await {
                Ok(stored) => stored,
                // A dangling alias means the row was removed outside JARVIS. Skipping it is the safe direction:
                // refusing the whole lookup would let one bad row hide every other candidate.
                Err(DatabaseError::EntityNotFound) => continue,
                Err(other) => return Err(other.into()),
            };
            let verified = is_verified_alias(alias.verification());
            let reply = alias_reply(alias);
            // One entry per **entity**, carrying every alias that matched it. Two aliases of one entity are one
            // candidate, and two rows would make a caller count them as two.
            if let Some(existing) = matches
                .iter_mut()
                .find(|existing| existing.entity.entity_id == stored.id().to_string())
            {
                existing.matched_aliases.push(reply);
                // The verdict is the **conjunction**: an entity is verified only if every alias that matched it
                // was. A verified name plus a guess is not an identity.
                existing.verified = existing.verified && verified;
            } else {
                let entity = self.reply_of(&stored).await?;
                matches.push(EntityMatchReply {
                    entity,
                    matched_aliases: vec![reply],
                    verified,
                });
            }
        }
        Ok(EntityLookupReply {
            returned: u32::try_from(matches.len()).unwrap_or(u32::MAX),
            matches,
        })
    }

    /// Attaches an alias to an entity, returning the entity with its aliases.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::NotFound`] when the entity does not exist, and
    /// [`EntityServiceError::Refused`] for an unknown verification or source kind, a verified alias from a
    /// source that cannot establish one, a confirmed probabilistic alias, or an alias another entity already
    /// holds as verified.
    pub async fn add_alias(
        &self,
        entity_id: &str,
        request: &AddAliasRequest,
    ) -> Result<EntityDetailReply, EntityServiceError> {
        let entity = find_entity(&self.database, entity_id).await?;
        let verification = parse_verification(&request.verification)?;
        let confidence = parse_confidence(&request.confidence)?;
        let source_kind: MemorySourceKind =
            request
                .source_kind
                .parse()
                .map_err(|_| EntityServiceError::UnknownValue {
                    field: "source_kind",
                    value: request.source_kind.clone(),
                })?;

        // The two cross-field rules `0009`'s `CHECK`s state, checked here so the refusal names **which rule**
        // rather than surfacing as a constraint failure. They are the schema's rules restated, not new ones —
        // and the first is why `MemorySourceKind::is_user_stated` exists: the nearest domain predicate
        // (`permitted_trust() == Authoritative`) admits `provider_record`, which the schema refuses.
        if verification == EntityMatch::Confirmed && !source_kind.is_user_stated() {
            return Err(EntityServiceError::Refused {
                reason: "alias_source_cannot_confirm",
                detail: "a confirmed alias must come from a user statement or a user correction"
                    .to_owned(),
            });
        }
        if verification == EntityMatch::Probabilistic && confidence == MemoryConfidence::Confirmed {
            return Err(EntityServiceError::Refused {
                reason: "alias_probabilistic_cannot_be_confirmed",
                detail: "a probabilistic alias is a candidate, so it cannot be held as confirmed"
                    .to_owned(),
            });
        }

        record_alias(
            &self.database,
            entity.id(),
            request.alias_kind.trim(),
            &request.alias_value,
            verification,
            confidence,
            source_kind,
            UtcTimestamp::now(&SystemClock),
        )
        .await
        .map_err(|error| match error {
            DatabaseError::AliasAlreadyVerified { existing_entity_id } => EntityServiceError::Refused {
                reason: "alias_already_verified",
                detail: format!(
                    "entity {existing_entity_id} already holds this alias as verified; merge the two entities \
                     or attach the alias to that one"
                ),
            },
            other => other.into(),
        })?;

        self.read(entity_id).await
    }

    /// Merges one entity into another, returning the winner.
    ///
    /// # Errors
    ///
    /// Returns [`EntityServiceError::NotFound`] when either entity is missing, and
    /// [`EntityServiceError::Refused`] for a self-merge, a merge across workspaces, or a source or target that
    /// is not active. The reasons come from the storage layer's own checks rather than being re-implemented, so
    /// each rule has one home.
    pub async fn merge(
        &self,
        source_id: &str,
        request: &MergeEntityRequest,
    ) -> Result<EntityDetailReply, EntityServiceError> {
        let source: EntityId = source_id
            .parse()
            .map_err(|_| EntityServiceError::UnknownValue {
                field: "entity_id",
                value: source_id.to_owned(),
            })?;
        let target: EntityId =
            request
                .target_id
                .parse()
                .map_err(|_| EntityServiceError::UnknownValue {
                    field: "target_id",
                    value: request.target_id.clone(),
                })?;

        merge_entities(
            &self.database,
            source,
            target,
            UtcTimestamp::now(&SystemClock),
        )
        .await
        .map_err(|error| match error {
            DatabaseError::InvalidEntityMerge { reason } => EntityServiceError::Refused {
                reason: "invalid_merge",
                detail: reason.to_owned(),
            },
            other => other.into(),
        })?;

        // The **winner**, because that is the entity that still denotes something: the source is now `merged`
        // and points here, so returning it would hand a caller a name whose claims belong to another.
        self.read(&target.to_string()).await
    }

    /// Builds one entity's reply, with its link count.
    async fn reply_of(&self, entity: &StoredEntity) -> Result<EntityReply, EntityServiceError> {
        Ok(EntityReply {
            entity_id: entity.id().to_string(),
            label: entity.label().to_owned(),
            kind: entity.kind().as_str().to_owned(),
            status: entity.status().as_str().to_owned(),
            confidence: entity.confidence().as_str().to_owned(),
            attributes: entity.attributes().map(ToOwned::to_owned),
            merged_into: entity.merged_into().map(|id| id.to_string()),
            created_at: entity.created_at().to_string(),
            version: entity.version(),
            linked_memories: self.link_count(entity.id()).await?,
        })
    }

    /// Counts the claims linked to an entity.
    async fn link_count(&self, entity_id: EntityId) -> Result<u32, EntityServiceError> {
        Ok(jarvis_storage::count_entity_memory_links(&self.database, entity_id).await?)
    }
}

/// Returns whether an alias's verification means "this name denotes this entity".
///
/// A positive list rather than `!= Probabilistic`, so a level added to the schema later must be classified
/// deliberately. The negative form's failure mode is that a new value silently counts as an identity, which is
/// the direction that invents one.
const fn is_verified_alias(verification: EntityMatch) -> bool {
    match verification {
        EntityMatch::Confirmed | EntityMatch::ProviderId | EntityMatch::ExactIdentifier => true,
        // A guess, and the architecture's rule is that several entities may carry one.
        EntityMatch::Probabilistic => false,
    }
}

fn alias_reply(alias: &StoredAlias) -> EntityAliasReply {
    EntityAliasReply {
        alias_id: alias.id().to_owned(),
        entity_id: alias.entity_id().to_string(),
        alias_kind: alias.alias_kind().to_owned(),
        alias_value: alias.alias_value().to_owned(),
        verification: alias.verification().as_str().to_owned(),
        confidence: alias.confidence().as_str().to_owned(),
    }
}

fn check_page(limit: u32) -> Result<(), EntityServiceError> {
    if limit == 0 || limit > MAX_MEMORY_PAGE {
        return Err(EntityServiceError::PageTooLarge {
            requested: limit,
            maximum: MAX_MEMORY_PAGE,
        });
    }
    Ok(())
}

fn parse_kind(value: &str) -> Result<jarvis_storage::EntityKind, EntityServiceError> {
    value.parse().map_err(|_| EntityServiceError::UnknownValue {
        field: "kind",
        value: value.to_owned(),
    })
}

fn parse_confidence(value: &str) -> Result<MemoryConfidence, EntityServiceError> {
    value.parse().map_err(|_| EntityServiceError::UnknownValue {
        field: "confidence",
        value: value.to_owned(),
    })
}

/// Parses a caller-stated alias verification.
///
/// A local parser rather than a `FromStr` on the domain enum, because `EntityMatch` has none — it is a storage
/// and retrieval vocabulary where values arrive from a row. The set is the schema's, written out for the reason
/// every vocabulary type here writes it out: a rename cannot silently change what a stored row means, and a
/// caller's string cannot silently come to mean something new.
fn parse_verification(value: &str) -> Result<EntityMatch, EntityServiceError> {
    match value {
        "confirmed" => Ok(EntityMatch::Confirmed),
        "provider_id" => Ok(EntityMatch::ProviderId),
        "exact_identifier" => Ok(EntityMatch::ExactIdentifier),
        "probabilistic" => Ok(EntityMatch::Probabilistic),
        _ => Err(EntityServiceError::UnknownValue {
            field: "verification",
            value: value.to_owned(),
        }),
    }
}

/// Returns whether a string is a plausible JSON document for the attributes column.
///
/// Deliberately **not** a JSON parse. The schema bounds the column's bytes and the wire carries the value as an
/// opaque string, so the rule here is only the bracket shape; a real parse belongs where the document is
/// interpreted, and performing one here would make this surface a second definition of what the attributes
/// mean. What it does fix is the one-byte case: the schema's `2..8192` byte bound accepts `"` alone, which is
/// not a document, and that failure would name a constraint rather than a field.
fn is_plausible_document(value: &str) -> bool {
    let trimmed = value.trim();
    (2..=8192).contains(&trimmed.len())
        && ((trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']')))
}
