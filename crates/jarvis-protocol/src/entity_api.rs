//! Versioned REST DTOs for the entity surface (`P4-016`).
//!
//! An entity is what a memory is **about**. `docs/architecture/memory-and-context.md` requires that every
//! claim name a subject, and `docs/api/contracts.md` lists entities among the surfaces a client needs — while
//! the shipped product had **no way to create, list, or look one up**, which is why `P4-008` records that "a
//! remember is still unreachable by a user of the shipped product". This module is the wire form of the
//! surface that removes that limit.
//!
//! # The three rules that shape these types
//!
//! **No workspace in a request.** As everywhere else in this crate: the workspace comes from the authenticated
//! session, and `deny_unknown_fields` makes an attempt to supply one a `422` rather than an ignored value. That
//! is what makes "client A's entity cannot be the subject of client B's claim" a transport property rather
//! than a check in each handler.
//!
//! **A confidence and a verification are the caller's to state; a workspace and an identifier are not.** The
//! schema refuses a `confirmed` alias from an `unverified` source ("the model guessed an email and we treat it
//! as established"), so the source kind travels with the verification and both are validated against each other
//! by the storage layer rather than defaulted here. What a caller *cannot* set is the entity's status: a new
//! entity is `active`, and archived, merged, and deleted are reached by their own operations.
//!
//! **A match is reported as a candidate, never as an answer.** The architecture's rule for ambiguous names is
//! that they "remain separate candidates", so a lookup by name returns **every** entity matching it together
//! with the aliases that matched, and a lookup by alias returns whether the match was **verified** or merely
//! probabilistic. A reply that returned one entity with no indication of how it was chosen would be a guess
//! presented as an identity — and every later claim would inherit it.

use serde::{Deserialize, Serialize};

/// The stable wire name of an entity's kind.
pub type EntityKindName = String;
/// The stable wire name of an entity's stored status.
pub type EntityStatusName = String;
/// The stable wire name of an alias's verification level.
pub type AliasVerificationName = String;

/// Request body for `POST /api/v1/entities`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEntityRequest {
    /// The canonical label, which is what a lookup by name matches.
    pub label: String,
    /// What kind of thing it is, by its stable name.
    pub kind: EntityKindName,
    /// How confidently it was established, by its stable name.
    ///
    /// Required rather than defaulted. A default would be the daemon choosing how sure the caller is, and the
    /// two plausible defaults are opposite errors: `confirmed` claims more than the caller said, and
    /// `unverified` discards a fact the caller stated. The value is the caller's to give.
    pub confidence: String,
    /// Normalized attributes as one JSON document, when the caller has them.
    ///
    /// A bounded JSON document rather than named columns, following the schema: its shape is the domain's to
    /// validate, and decomposing it would give the same facts two homes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<String>,
}

/// One entity, as the surface reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityReply {
    /// The entity's identifier, which is what a remember names as its subject.
    pub entity_id: String,
    /// Its canonical label.
    pub label: String,
    /// What kind of thing it is.
    pub kind: EntityKindName,
    /// Its stored status.
    pub status: EntityStatusName,
    /// How confidently it was established.
    pub confidence: String,
    /// Its normalized attributes document, absent when none was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<String>,
    /// The entity this one was merged into, present only for a merged entity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_into: Option<String>,
    /// When it was created, as an RFC 3339 instant.
    pub created_at: String,
    /// The version a later mutation must present.
    ///
    /// Carried for the same reason a memory reference carries one: a guard whose value a client cannot obtain
    /// is a guard the client cannot pass, and a merge names two entities by identifier plus this.
    pub version: i64,
    /// How many claims in this workspace are linked to it.
    ///
    /// The one figure that answers "is this entity used", which is what an operator merging duplicates needs:
    /// merging the wrong one moves no links, so the count is evidence rather than decoration.
    pub linked_memories: u32,
}

/// Reply body for `POST /api/v1/entities` and `GET /api/v1/entities/{id}`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityDetailReply {
    /// The entity.
    #[serde(flatten)]
    pub entity: EntityReply,
    /// The aliases attached to it, oldest first.
    pub aliases: Vec<EntityAliasReply>,
}

/// Reply body for `GET /api/v1/entities`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityListReply {
    /// The entities, newest first.
    pub entities: Vec<EntityReply>,
    /// How many were returned.
    pub returned: u32,
    /// The bound that was applied, so a caller can tell a complete listing from a truncated one.
    pub limit: u32,
}

/// Request body for `POST /api/v1/entities/{id}/aliases`.
///
/// # Why the source kind is a request field here and not for a memory
///
/// A memory's source kind is derived from what the caller is doing — a remember states it, a summary is fixed
/// at `document` — but an alias's is a fact about **who established the alias**, and the schema's rule
/// ("a verified alias cannot come from an unverified source") is a relationship between two fields the caller
/// supplies. Deriving either one would make the rule unstateable, and the pair is exactly what a resolver needs
/// to know how much the name means.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddAliasRequest {
    /// The alias kind: `name`, `email`, `handle`, `phone`, `provider_id`, `url`, or `other`.
    pub alias_kind: String,
    /// The alias value as the operator states it.
    pub alias_value: String,
    /// How the alias was established: `confirmed`, `provider_id`, `exact_identifier`, or `probabilistic`.
    pub verification: AliasVerificationName,
    /// Where it came from: `user_statement`, `user_correction`, `provider_record`, `tool_observation`,
    /// `document`, `model_inference`, or `external_content`.
    pub source_kind: String,
    /// How confidently it matches, by its stable name.
    pub confidence: String,
}

/// One alias, as the surface reports it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityAliasReply {
    /// The alias's identifier.
    pub alias_id: String,
    /// The entity it names.
    pub entity_id: String,
    /// Its kind.
    pub alias_kind: String,
    /// The value as it was stated, before normalization.
    pub alias_value: String,
    /// How it was established.
    pub verification: AliasVerificationName,
    /// How confidently it matches.
    pub confidence: String,
}

/// Reply body for a lookup, by label or by alias.
///
/// # Why the answer is a list of candidates with their evidence
///
/// "Ambiguous aliases remain separate candidates" is a rule about what a resolver may conclude, so the reply
/// carries every match and the aliases that produced it. A caller with one match has its answer; a caller with
/// several has the material to disambiguate rather than a first row it cannot question. `verified` is on the
/// reply rather than left for the caller to infer from the alias list, because "this name resolved" and "this
/// name is one of several guesses" are different answers to the same question.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityLookupReply {
    /// Every entity matching the query, with the aliases that matched.
    pub matches: Vec<EntityMatchReply>,
    /// How many were returned.
    pub returned: u32,
}

/// One entity that matched, with the aliases that matched it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EntityMatchReply {
    /// The entity.
    #[serde(flatten)]
    pub entity: EntityReply,
    /// The aliases that matched the query. Empty when the match was on the entity's own label.
    pub matched_aliases: Vec<EntityAliasReply>,
    /// Whether every alias that matched was **verified**.
    ///
    /// `false` when the only matches were `probabilistic`, which is the value that says "this is a candidate
    /// rather than an identity". A caller that needs an identity must check it; a caller that wanted candidates
    /// now has them.
    pub verified: bool,
}

/// Request body for `POST /api/v1/entities/{id}/merge`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MergeEntityRequest {
    /// The entity to merge **into**, which must be active.
    ///
    /// Named in the body rather than in the path because the operation has two subjects and only one of them
    /// can be the route's. Putting the winner in the path would make "merge A into B" read as "merge B from A",
    /// which is the direction an operator is most likely to get wrong — and getting it wrong moves every link
    /// the wrong way.
    pub target_id: String,
}
