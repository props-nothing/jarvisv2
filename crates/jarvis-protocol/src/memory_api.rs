//! Versioned REST DTOs for the memory read and lifecycle surface.
//!
//! `docs/api/contracts.md` sketches the memory repository as `get`/`search`/`correct`/`forget` plus a
//! `DeletionReceipt`, and `docs/architecture/memory-and-context.md` states the retention and privacy rules
//! these shapes exist to make usable. This module is the **wire** form of that surface, so the same split
//! holds as elsewhere in this crate: a change to a domain struct does not silently change a
//! client-visible contract.
//!
//! # Three rules that shape every type here
//!
//! **No scope in a request body.** A memory belongs to a workspace, and a caller cannot name one — the
//! workspace comes from the authenticated session, and `deny_unknown_fields` makes an attempt to supply one
//! a `422` rather than an ignored value. That is the same rule the tool-call body's absent `workspace_id`
//! follows, and it is what makes "client A's memory cannot enter client B's context" a property of the
//! transport rather than of each handler.
//!
//! **A reference never carries content.** Every reply identifies a memory by its identifier, its type, its
//! source kind, and its timestamps. Retrieval's own contract is that a context item carries an opaque
//! reference rather than text, so a listing that returned content would be a second, less careful path into
//! a prompt. `GET /memories/{id}` returns the text because a user asking to see one claim is the one case
//! where content is the answer.
//!
//! **A request that changes state names the memory and its version.** `expected_version` is required on
//! correct and forget, because those are the two operations where a lost update is not merely confusing: a
//! correction applied to the wrong claim, or a deletion that raced an edit, are both unrecoverable without
//! an audit read. This is deliberately the opposite of the cancellation decision in `ADR-0022` — see
//! `docs/api/contracts.md`'s note that a version guard belongs only where the subject of the write is not
//! already named by a durable identity with its own transition rule.

use serde::{Deserialize, Serialize};

/// The stable wire name of a memory type, so a client and the daemon cannot disagree about the spelling.
///
/// A `String` rather than a re-exported domain enum, because the wire form is a contract with clients while
/// the domain enum is a closed set the daemon owns. The daemon converts, and an unknown value is a `422`
/// naming the field rather than a silently defaulted type — the default would be the *most* permissive
/// reading of a value the caller mistyped.
pub type MemoryTypeName = String;
/// The stable wire name of a memory's source kind.
pub type MemorySourceKindName = String;
/// The stable wire name of a memory's stored status.
pub type MemoryStatusName = String;
/// The stable wire name of an effective status, which accounts for expiry.
pub type EffectiveStatusName = String;

/// Request body for `POST /api/v1/memories`.
///
/// # Why a remember request carries a source and not a confidence
///
/// The caller states **where the claim came from**; the daemon decides what confidence that source can
/// support and what sensitivity its content requires. Letting a caller supply a confidence would let it
/// assert `confirmed` about a model inference, which is the one thing `P4-001` and `P4-003` exist to
/// prevent — the caps are constructor rules `jarvis_core` enforces, and taking the value from the wire would
/// put the cap in the caller's hands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RememberRequest {
    /// The claim's text. Bounded by the memory contract.
    pub content: String,
    /// What kind of memory this is, by its stable name.
    pub memory_type: MemoryTypeName,
    /// Where the claim came from, by its stable name.
    pub source_kind: MemorySourceKindName,
    /// How important the claim is, as a bounded rank.
    ///
    /// Optional so a caller that does not care need not guess. The default is the middle of the range
    /// rather than the top, because a default that outranks user statements would make the ranking's own
    /// importance signal meaningless.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<u8>,
    /// The entities the claim is about, by their stored identifier.
    ///
    /// Required in practice even though the type does not encode it: a claim must be *about* something, so a
    /// request with an empty list is refused with a `422` rather than defaulted. The earlier version of this
    /// note said the daemon would attach the workspace's own subject entity, which would have been a
    /// placeholder standing in for an answer the caller did not give — and a claim filed against a
    /// placeholder is indistinguishable from one the user meant, so the refusal is the honest behaviour.
    ///
    /// There is no entity *resolution* yet (`P4-008` limit): the caller must already hold an entity
    /// identifier, which it obtains from the entities surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entity_ids: Vec<String>,
    /// The claim in normalized subject/predicate/object form, when the caller has one.
    ///
    /// All three or none: the pipeline refuses a partial triple, and `deny_unknown_fields` plus a single
    /// optional struct makes "two of three" unrepresentable rather than merely refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimBody>,
    /// The memory this claim **corrects**, when it corrects one.
    ///
    /// Declared rather than inferred, for the reason `ADR-0045` records: the search key *is* the sorted word
    /// set, so a claims-sharing-a-key pair is the same claim and a different claim has no stored
    /// counterpart to compare against. Only the caller knows the two are about the same subject area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

/// The normalized form of a claim: three parts, all or none.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimBody {
    /// The claim's subject.
    pub subject: String,
    /// The claim's predicate.
    pub predicate: String,
    /// The claim's object.
    pub object: String,
}

/// Reply body for a remember, correct, or forget.
///
/// # Why one shape for three operations
///
/// All three answer the same question — *what is the state of this memory now* — and the answers are the
/// same three facts: what it is, whether it is current, and what happened to it. Three shapes would let a
/// client's handling diverge on a distinction that does not exist, and the outcome field already names which
/// operation produced it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryReply {
    /// The memory's identifier.
    pub memory_id: String,
    /// What kind of memory it is.
    pub memory_type: MemoryTypeName,
    /// Its stored status, which is what a human set.
    pub status: MemoryStatusName,
    /// Its effective status at the time of the reply, which accounts for expiry.
    ///
    /// Separate from `status` because the two differ for an expired-but-active claim, and a client showing
    /// only the stored value would present a lapsed claim as current.
    pub effective_status: EffectiveStatusName,
    /// What the operation did, by its stable name.
    pub outcome: String,
    /// The version after the operation, for a subsequent correct or forget.
    pub version: i64,
    /// Why, when the operation changed what was retrievable or refused to.
    ///
    /// Present for a refusal and for a correction, absent for a plain remember. A refusal that says only
    /// "refused" sends the caller looking for a policy problem that may not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One memory in a listing: a reference, never the content.
///
/// # Why this type has no `content` field
///
/// The listing exists to answer "what does JARVIS remember, and can I trust it" without putting text into a
/// place text is not expected. A reference plus the reasons a claim has its current standing is what the
/// question needs, and `GET /memories/{id}` is where the text belongs — one claim, deliberately requested.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryReference {
    /// The memory's identifier.
    pub memory_id: String,
    /// What kind of memory it is.
    pub memory_type: MemoryTypeName,
    /// Where the claim came from.
    pub source_kind: MemorySourceKindName,
    /// Its stored status.
    pub status: MemoryStatusName,
    /// Its effective status at the time of the listing.
    pub effective_status: EffectiveStatusName,
    /// How much it matters.
    pub importance: u8,
    /// How well supported it is.
    pub confidence: String,
    /// Its sensitivity.
    pub sensitivity: String,
    /// When it was recorded, as an RFC 3339 instant.
    pub created_at: String,
    /// When it was last changed.
    pub updated_at: String,
    /// When it was last selected into a context, absent if never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<String>,
    /// How often it has been usefully retrieved.
    pub retrieval_count: u32,
    /// The identity that accepted it, absent when no acceptance was recorded.
    ///
    /// `P4-014`: "admission is a decision that names its approver", and a client asking "who decided this
    /// claim is true" has to be able to answer it. Absent means one of two things the status distinguishes:
    /// the claim never needed accepting, or it was accepted before admission was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_by_actor_id: Option<String>,
    /// When that acceptance was recorded, alongside [`Self::admitted_by_actor_id`].
    ///
    /// Present exactly when the approver is, because the domain refuses half a decision and a reply that
    /// could carry one would present the other half as complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_at: Option<String>,
    /// The version a correction or deletion must present.
    ///
    /// # Why this is on the reference rather than only on a write's reply
    ///
    /// The guard on `correct` and `forget` requires a version the caller observed, and the caller observes it
    /// by reading. A reply that did not carry one would leave a client with no way to obtain the value it must
    /// send — it would have to re-read and hope, which is exactly the lost update the guard exists to prevent.
    pub version: i64,
    /// The memory that superseded it, present only for a superseded claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

/// Reply body for `GET /api/v1/memories`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryListReply {
    /// The references, newest first.
    pub memories: Vec<MemoryReference>,
    /// How many were returned.
    pub returned: u32,
    /// The bound that was applied, so a caller can tell a complete listing from a truncated one.
    ///
    /// A caller that cannot see the bound reads a truncated list as the whole answer, which for "what do
    /// you remember about me" is a materially wrong answer rather than an incomplete one.
    pub limit: u32,
}

/// Reply body for `GET /api/v1/memories/{id}`, which is the one place content is returned.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryDetailReply {
    /// The reference fields.
    #[serde(flatten)]
    pub reference: MemoryReference,
    /// The claim's text, as stored.
    pub content: String,
    /// The normalized claim, when the memory has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimBody>,
    /// The source's locator.
    pub source_locator: String,
    /// The entities the claim is about, by identifier.
    pub entity_ids: Vec<String>,
    /// Whether this claim may be offered to a user as established.
    ///
    /// The single predicate a presentation site should ask, computed by the domain so a client cannot apply
    /// one of its two conditions and forget the other: a superseded claim and an unconfirmed one are both
    /// "not established", for different reasons.
    pub is_stated_as_fact: bool,
    /// Whether a retrieved use of this claim would be fenced as untrusted data.
    ///
    /// Reported because it is the difference between a claim the model reads as the user's own words and one
    /// it reads as quoted external content, and a user inspecting their own memory is entitled to know which.
    pub carries_untrusted_trust: bool,
}

/// Request body for `POST /api/v1/memories/search`, which is a `POST` so the query is a body.
///
/// # Why a POST rather than a query string
///
/// A search term is user content. `docs/architecture/security.md` treats a query parameter as a place a
/// credential must never appear, and the same reasoning applies to content: a URL reaches access logs,
/// browser history, and `Referer` headers, and a memory search term is exactly the kind of phrase that
/// should not. It is also the field that would have to be percent-encoded and re-validated, which is a
/// second parsing path for a value the body already carries safely.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySearchRequest {
    /// The text to match against claims.
    ///
    /// Absent means "no text constraint", which is the listing case. Present-and-empty is refused rather
    /// than treated as absent, because a caller that sent an empty string asked for something and a silent
    /// widening of the result is the wrong reading of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Restrict to these memory types, by stable name. Empty means every eligible type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_types: Vec<MemoryTypeName>,
    /// Restrict to claims about these entities. Empty means no entity constraint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entity_ids: Vec<String>,
    /// Require at least this much trust, by stable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_trust: Option<String>,
    /// Require at least this much confidence, by stable name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_confidence: Option<String>,
    /// How many results to return at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Reply body for the memory search.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemorySearchReply {
    /// The matches, best first.
    pub matches: Vec<MemorySearchHit>,
    /// How many claims were considered before ranking.
    pub considered: u32,
    /// The bound that was applied.
    pub limit: u32,
}

/// One ranked match, with the score and the reason that produced it.
///
/// # Why the answer carries a score and a reason
///
/// `docs/architecture/memory-and-context.md` requires that "a user can ask why something was remembered or
/// used" and be answered "from stored provenance and selection reasons, not generate an explanation after
/// the fact". A search that returned only identifiers would make the ranking unexplainable from stored data,
/// and a client that computed its own explanation would be generating one. The score here **is** the
/// arithmetic that produced the order, and the reason is the component that won the precedence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemorySearchHit {
    /// The reference fields.
    #[serde(flatten)]
    pub reference: MemoryReference,
    /// The weighted total, out of the ranking's own scale.
    pub score: u32,
    /// Why this claim was selected, by stable name.
    pub reason: String,
    /// Whether the reason means the claim actually matched the query.
    ///
    /// A claim included for recency did not match anything; one included for a keyword did. Collapsing the
    /// two would let a memory that answered nothing be presented as an answer, which is why the domain
    /// exposes the distinction and why it is on the wire.
    pub is_a_match: bool,
    /// Every signal's own contribution, by signal name.
    ///
    /// The stored components rather than a recomputation: the ranking's doc says the total *is* the sum of
    /// these, so an operator reading them is reading the arithmetic that produced the order.
    pub contributions: Vec<SignalContribution>,
}

/// One signal's weighted contribution to a total.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignalContribution {
    /// The signal's stable name.
    pub signal: String,
    /// Its weighted contribution to the total.
    pub contribution: u32,
}

/// Reply body for `GET /api/v1/memories/export`.
///
/// # Why the export carries content
///
/// `docs/architecture/memory-and-context.md` requires "each memory type has configurable retention and
/// export behavior", and an export whose text was redacted would not be one. This is the deliberate
/// exception to the reference-only rule: the user asked for their own data, which is the case `GDPR`-shaped
/// portability exists for.
///
/// The envelope states what it **excludes**, because an archive that looks complete and is not is worse than
/// one that says what it left out.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryExportReply {
    /// The workspace the export covers.
    pub workspace_id: String,
    /// When the export was produced, as an RFC 3339 instant.
    pub exported_at: String,
    /// The claims, newest first.
    pub memories: Vec<ExportedMemory>,
    /// How many claims the export contains.
    pub count: u32,
    /// What the export deliberately does not contain.
    ///
    /// A list rather than a boolean, because each entry is a different reason a reader might otherwise
    /// assume completeness: deleted claims leave a tombstone and no text, superseded claims are retained,
    /// and provider-side copies are outside this platform's reach entirely.
    pub exclusions: Vec<String>,
}

/// One exported claim, with its text and provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportedMemory {
    /// The memory's identifier.
    pub memory_id: String,
    /// What kind of memory it is.
    pub memory_type: MemoryTypeName,
    /// The claim's text.
    pub content: String,
    /// Where the claim came from.
    pub source_kind: MemorySourceKindName,
    /// The source's locator.
    pub source_locator: String,
    /// Its stored status.
    pub status: MemoryStatusName,
    /// Its effective status at export time.
    pub effective_status: EffectiveStatusName,
    /// How well supported it is.
    pub confidence: String,
    /// Its sensitivity.
    pub sensitivity: String,
    /// How much it matters.
    pub importance: u8,
    /// The normalized claim, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<ClaimBody>,
    /// The entities it is about, by identifier.
    pub entity_ids: Vec<String>,
    /// When it was recorded.
    pub created_at: String,
    /// When it was last changed.
    pub updated_at: String,
}

/// Request body for `POST /api/v1/memories/{id}/correct`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectMemoryRequest {
    /// The replacement claim's text.
    pub content: String,
    /// The version the caller observed, which the write is guarded against.
    ///
    /// Required rather than optional, and the asymmetry with cancellation is deliberate. A cancellation is
    /// a request about a run whose own terminal-state rule already refuses a second one, so a version adds
    /// nothing but a refusal (`ADR-0022`). A correction changes what will be retrieved as current truth, and
    /// there is exactly one right subject for it — the claim the user was looking at — so a correction
    /// applied to a claim the user has not read is a change to their memory they did not ask for.
    pub expected_version: i64,
    /// Keep the replacement's entities, when the caller wants to restate them.
    ///
    /// Absent means "inherit from the claim being corrected", which is the common case: a corrected
    /// preference is still about the same subject. Present-and-empty is refused by the daemon, because a
    /// memory with no entity is not storable and a silent fallback would hide the caller's mistake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_ids: Option<Vec<String>>,
}

/// Request body for `POST /api/v1/memories/{id}/forget`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForgetMemoryRequest {
    /// The version the caller observed.
    pub expected_version: i64,
    /// Whether to also remove the tombstone that blocks a re-ingest.
    ///
    /// # Why this is an explicit request and not a separate verb
    ///
    /// The default is `false`, which is the safe direction: a tombstone exists so a claim the user deleted
    /// does not return through a later ingest, and a re-ingest that resurrected it would be a deletion that
    /// did not hold. Asking for `true` is a deliberate statement that the user wants the claim to be
    /// re-learnable — an undo, not a cleanup — and it is recorded in the reply so the audit trail shows
    /// which of the two happened.
    #[serde(default)]
    pub allow_relearn: bool,
}

/// Request body for `POST /api/v1/memories/{id}/confirm`.
///
/// # Why the version is the only field
///
/// `P4-014` is "admission is a decision that names its approver", so the obvious shape is an
/// `approver_actor_id` field beside the version. It is deliberately absent: the approver is read from the
/// daemon's seeded identity, because a caller that could name its own approver could accept its own claim by
/// naming somebody else. The guard that refuses a self-admission compares the approver against the claim's
/// author, and it is only meaningful if neither side comes from the request.
///
/// The version is present for the same reason every other write carries it: an acceptance decided against a
/// stale read would confirm a version of the claim the reviewer never saw.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmMemoryRequest {
    /// The version the caller observed.
    pub expected_version: i64,
}

/// Summary of what a deletion removed.
///
/// `docs/api/contracts.md` names this `DeletionReceipt`, and the name is the point: it is the evidence a
/// privacy obligation was met, so it counts what was removed by **category** and states what it could not
/// reach. A receipt that said only "deleted" would be indistinguishable from one for a claim that never
/// existed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeletionReceipt {
    /// The memory that was deleted.
    pub memory_id: String,
    /// Its text length in characters, so the caller can see that text was removed rather than assume it.
    ///
    /// A length and not the text: the receipt is evidence, and evidence that quotes the removed content is
    /// the content surviving the deletion in a second table.
    pub removed_content_chars: u32,
    /// Whether the derived search key was removed.
    ///
    /// Reported rather than assumed because the key holds the claim's words, so "the text is gone" is only
    /// true if the key went with it. It is the field that makes the acceptance invariant auditable.
    pub removed_search_key: bool,
    /// Whether a tombstone was written, blocking a re-ingest of the same claim.
    pub tombstone_written: bool,
    /// How many entity links were removed.
    ///
    /// A count rather than a boolean, because a claim can be about several entities and "three links went"
    /// is checkable against what the caller stored, while "links were handled" is not.
    pub removed_entity_links: u32,
    /// Whether the claim was superseding another, and that link was cleared.
    pub cleared_supersession: bool,
    /// What the deletion could not reach, named.
    ///
    /// Never empty, because there is always at least one scope outside this platform's reach: a provider that
    /// received the claim as context holds its own copy under its own retention policy, and
    /// `docs/architecture/memory-and-context.md` requires that be "surfaced separately" rather than implied.
    pub unreachable: Vec<String>,
}
