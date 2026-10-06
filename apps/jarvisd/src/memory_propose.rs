//! The tool a **model** submits a memory candidate through.
//!
//! `P4-014`: "the model may propose candidates, labels, confidence, and entities. Deterministic code validates
//! shape, source linkage, workspace, size, sensitivity, and retention." Before this adapter, no tool touched
//! memory at all, so that sentence described an intention rather than a capability — the model had no way to
//! propose anything, which is also why the self-admission rule it implies had no subject.
//!
//! # What the model can and cannot say
//!
//! The arguments are deliberately narrower than `RememberRequest`, and every omission is a load-bearing one:
//!
//! - **No `source_kind`.** The source is [`MemorySourceKind::ModelInference`], always, and it is set here
//!   rather than accepted. A model able to declare its own claim a `user_statement` could record its own
//!   output as something the person said — which is the exact confusion the inference boundary exists to
//!   prevent, and it would arrive through a field rather than through a bug.
//! - **No `confidence`.** The candidate proposes the ceiling the source permits, and `MemoryCandidate::admit`
//!   caps it again. A model asserting confidence is the thing `MemoryRecord::new` refuses outright, so offering
//!   the field would be offering one that is always refused.
//! - **No `source_locator`.** It is derived from the run, so the claim is traceable to what produced it. A
//!   model choosing its own provenance would make "where did this come from" a model-authored answer.
//! - **No `supersedes`.** A model cannot retire an existing claim by proposing a correction: `compare` treats a
//!   differing claim at the same key as a correction, and a correction supersedes. That is right for a *person*
//!   correcting their own memory and wrong for a model, which would otherwise be able to replace a user's
//!   statement with its own inference.
//!
//! So the model may propose **content**, a **type**, the **entities** it is about, and its **importance**. Every
//! other field that decides what the claim means is derived by deterministic code, which is the architecture's
//! division of labour rather than a restriction invented here.
//!
//! # Why the result is an outcome rather than a boolean
//!
//! A proposal can be admitted, refused, or found already present, and `MemoryAdmission` already distinguishes
//! those. Reporting `true` would lose the difference between "the workspace now holds your candidate" and "the
//! workspace already held this claim", and a model told only the first would believe it had added something.

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{
    CorrelationId, MemoryCandidate, MemoryConfidence, MemorySourceKind, MemoryType, Sensitivity,
    SystemClock, UtcTimestamp,
};
use jarvis_storage::SqliteDatabase;
use jarvis_tools::{
    AdapterError, ApprovalPolicy, Availability, BoundedOutput, EffectSet, Idempotency,
    ProviderEvidence, RetryDeclaration, SchemaError, Scope, ScopeError, ScopeSet, ToolCallResult,
    ToolDefinition, ToolDefinitionError, ToolDefinitionParts, ToolEffect, ToolExecutionRequest,
    ToolExecutor, ToolId, ToolIdError, ToolOutcomeRecord, ToolSchema, ToolSensitivity, ToolSource,
};
use thiserror::Error;

use crate::memory_service::{MemoryService, MemoryServiceError};

/// The canonical identifier the model requests.
pub const PROPOSE_TOOL: &str = "jarvis.memory.propose";

/// The scope a caller must hold for the tool to be authorized at all.
///
/// Distinct from `files.read` so a workspace can grant "propose a memory" without granting "read a file".
/// `memory.propose` is the narrower capability: it writes a claim nobody has confirmed, and reading it back
/// through a model's context is a separate decision the retrieval path makes.
pub const PROPOSE_SCOPE: &str = "memory.propose";

const TIMEOUT_SECONDS: u32 = 15;

/// Why this adapter could not state its own contract.
///
/// Four variants rather than one opaque source, because the four are different authoring mistakes: a malformed
/// identifier, a schema that is not valid JSON Schema, a scope name outside the grammar, and a definition whose
/// declared risk disagrees with its effects. A single "the definition was refused" would leave a reader to
/// guess which constant to look at, which is the diagnostic gap this repository keeps finding.
#[derive(Debug, Error)]
pub enum ProposeToolError {
    /// The canonical identifier was rejected.
    #[error("the proposal tool's identifier was rejected: {0}")]
    Identifier(#[from] ToolIdError),
    /// A schema was rejected.
    #[error("the proposal tool's schema was rejected: {0}")]
    Schema(#[from] SchemaError),
    /// The required scope was rejected.
    #[error("the proposal tool's scope was rejected: {0}")]
    Scope(#[from] ScopeError),
    /// The definition was rejected as a whole.
    #[error("the proposal tool's definition was rejected: {0}")]
    Definition(#[from] ToolDefinitionError),
}

/// The input contract.
///
/// `additionalProperties: false` is the enforcement of the module's "what the model cannot say" list: without
/// it a model could send `source_kind` and the parser would ignore it, which reads as accepted.
const INPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["content", "memory_type"],
  "properties": {
    "content": {
      "type": "string",
      "minLength": 1,
      "maxLength": 4096,
      "description": "The claim to remember, in the user's own terms."
    },
    "memory_type": {
      "type": "string",
      "enum": [
        "working", "conversation", "episodic", "semantic",
        "preference", "relationship", "procedural"
      ],
      "description": "What kind of claim this is. A relationship claim is stored for review, never as fact."
    },
    "entity_ids": {
      "type": "array",
      "minItems": 1,
      "maxItems": 16,
      "items": { "type": "string", "minLength": 1 },
      "description": "Leave this out for a claim about the user, which is almost always what is wanted. Only to name other existing subjects, by identifier."
    },
    "importance": {
      "type": "integer",
      "minimum": 0,
      "maximum": 4,
      "description": "How much it matters, when the model has a reason to say. Defaults to the middle."
    }
  }
}"#;

/// The output contract.
const OUTPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["outcome", "status"],
  "properties": {
    "outcome": {
      "type": "string",
      "enum": ["proposed", "already_remembered", "already_superseded"],
      "description": "What happened. `proposed` means a new claim is held for review."
    },
    "status": {
      "type": "string",
      "enum": ["proposed", "active", "archived", "deleted"],
      "description": "The status the stored claim holds. A model inference is always `proposed`."
    },
    "memory_id": {
      "type": "string",
      "description": "The claim's identifier, when one was stored or already existed."
    },
    "detail": {
      "type": "string",
      "description": "A bounded explanation of a refusal, when the candidate was refused."
    }
  }
}"#;

/// Submits a memory candidate on a model's behalf.
///
/// Holds the same [`MemoryService`] the HTTP surface uses rather than a second implementation, so a candidate
/// submitted by a model and one submitted over the wire pass through **one** admission path. A separate path
/// is how the two come to disagree about what a claim of a given type means, which is the reason the service's
/// own module documentation gives for routing remembers through the pipeline at all.
pub struct MemoryProposeTool {
    service: MemoryService,
}

impl MemoryProposeTool {
    /// Builds the adapter over the daemon's database.
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self {
            service: MemoryService::new(database),
        }
    }

    /// Builds the canonical definition of this adapter's tool.
    ///
    /// # Errors
    ///
    /// Returns [`ProposeToolError`] naming the constant that was rejected.
    pub fn definition() -> Result<ToolDefinition, ProposeToolError> {
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(PROPOSE_TOOL)?,
            version: "1.0.0".to_owned(),
            title: "Propose a memory".to_owned(),
            description: "Proposes something worth remembering about the user or their work. The claim is \
                          validated and stored for review: it is never presented as established, and a \
                          person decides whether it becomes trusted."
                .to_owned(),
            input_schema: ToolSchema::parse(INPUT_SCHEMA)?,
            output_schema: ToolSchema::parse(OUTPUT_SCHEMA)?,
            // **`write`, not `read_only`.** A proposal persists: it is stored, it is listed, and it can be
            // retrieved by the person who reviews it. The effect says what the call does to the world rather
            // than how much authority the claim carries, and a claim nobody may act on is still a row.
            effects: EffectSet::single(ToolEffect::Write),
            // Risk 1 is the floor `Write` carries. The claim is not established, it is reversible, and it is
            // scoped so a workspace can withhold the capability entirely — but it is a durable write and the
            // risk says so rather than rounding down to a read.
            risk: 1,
            required_scopes: ScopeSet::single(Scope::new(PROPOSE_SCOPE)?),
            // `Auto`: the write cannot mislead anybody by itself. The claim is stored `proposed`, the retrieval
            // path excludes a model inference at any status (`ADR-0049` §6), and the inference boundary caps it
            // at `unverified` — so an unreviewed proposal changes nothing a user reads. Requiring approval
            // would ask a person to authorize a write whose only effect is to appear in the review list, and
            // the review *is* the approval step.
            approval: ApprovalPolicy::Auto,
            timeout_seconds: TIMEOUT_SECONDS,
            // A blind retry is safe because the write is idempotent on its key: a re-statement of the same
            // claim about the same entity is answered `already_remembered` rather than stored twice.
            retry: RetryDeclaration {
                attempts: 2,
                backoff_ceiling_seconds: 1,
            },
            idempotency: Idempotency::Required,
            source: ToolSource::Native,
            availability: Availability::Available,
            // A model's proposal is `Internal` at most. Declaring `Public` would let a claim about a person
            // reach any destination before anybody had read it, and this adapter cannot know what the content
            // implies — the sensitivity floor from the claim's *type* is the pipeline's decision, and this
            // ceiling only says "not public" until then.
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })?)
    }
}

impl std::fmt::Debug for MemoryProposeTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names the adapter without printing the service, which holds a database handle.
        formatter
            .debug_struct("MemoryProposeTool")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolExecutor for MemoryProposeTool {
    fn adapter_id(&self) -> &'static str {
        "memory-propose"
    }

    async fn execute(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        // The tool is matched on its canonical identifier, and an unknown one is refused. A
        // `NotImplemented` for an unrecognized tool is what makes a registry mistake visible instead of silent.
        if request.tool().to_string() != PROPOSE_TOOL {
            return Err(AdapterError::NotImplemented {
                tool: request.tool().to_string(),
            });
        }
        let now = UtcTimestamp::now(&SystemClock);
        if request.is_past_deadline(now) {
            return Err(AdapterError::RefusedBeforeReaching {
                reason: "the call was already past its deadline".to_owned(),
            });
        }

        let arguments = match ProposalArguments::parse(request.arguments()) {
            Ok(arguments) => arguments,
            Err(reason) => return Err(AdapterError::RefusedBeforeReaching { reason }),
        };

        match self.submit(&arguments, request.correlation_id(), now).await {
            Ok(admitted) => confirm(
                &admitted.outcome,
                &admitted.stored.memory.record().id().to_string(),
                now,
            ),
            // **A refusal is the tool working, not the tool failing.** The pipeline decided the candidate
            // must not be stored — a provider that cannot back a preference, a tombstoned claim — and that
            // decision is an answer the model can act on. Reporting it as `Failed` would tell a model the
            // platform was broken when the platform had judged its input.
            Err(MemoryServiceError::Refused { reason, detail }) => {
                Ok(refusal(reason, &detail, now))
            }
            // **An unusable argument is the same kind of answer, and this arm is the one that matters most
            // in practice.** An entity the workspace does not have arrives as `UnknownValue` rather than as
            // `Refused`, because the HTTP surface reports it as a `422` naming the field the caller must fix.
            // For a model it is the same judgement — "this identifier names nothing here" — and it is the
            // *common* case rather than an edge one, since a model proposing a subject it only inferred will
            // name one that does not exist. Reporting it as an adapter fault would turn the most likely
            // outcome of a correct proposal into a platform error.
            Err(MemoryServiceError::UnknownValue { field, value }) => Ok(refusal(
                "unusable_argument",
                &format!("the {field} argument is not usable here: {value}"),
                now,
            )),
            // A storage failure is a real failure: the write was attempted and did not land. It is not
            // `Unknown`, because nothing durable was left behind that a retry could duplicate — the row either
            // exists or it does not, and the insert is the thing that failed.
            Err(MemoryServiceError::Storage(error)) => Err(AdapterError::ProviderRefused {
                reason: format!("the claim could not be stored: {error}"),
            }),
            // The remaining variants are request-shaped failures this adapter's own construction makes
            // unreachable: it never pages, never looks a claim up by identifier, and never writes against a
            // version. Spelled out rather than wildcarded so a new variant is a compile error here rather than
            // silently reported as something it is not.
            Err(
                error @ (MemoryServiceError::PageTooLarge { .. }
                | MemoryServiceError::NotFound
                | MemoryServiceError::Conflict),
            ) => Err(AdapterError::RefusedBeforeReaching {
                reason: format!("the proposal was refused at {}", field_or_kind(&error)),
            }),
        }
    }
}

/// Stores the candidate through the service's admission path.
impl MemoryProposeTool {
    async fn submit(
        &self,
        arguments: &ProposalArguments,
        correlation_id: CorrelationId,
        now: UtcTimestamp,
    ) -> Result<crate::memory_service::Admitted, MemoryServiceError> {
        let candidate = MemoryCandidate {
            workspace_id: self.service.workspace().await?,
            content: arguments.content.clone(),
            classification: jarvis_core::CandidateClassification {
                memory_type: arguments.memory_type,
                // **The boundary, in one line.** Whatever the model believes about its claim, the source is
                // its own inference, which is what makes the pipeline require a proposal and cap the
                // confidence. Everything else in this adapter exists to make this line true.
                source_kind: MemorySourceKind::ModelInference,
            },
            proposed_sensitivity: Sensitivity::Public,
            proposed_confidence: MemoryConfidence::Unverified,
            importance: arguments.importance,
            proposed_entities: Vec::new(),
            structured_claim: None,
            // Derived from the correlation identity rather than accepted, so the claim is traceable to the
            // call that produced it and a model cannot choose its own provenance.
            source_locator: format!("tool:{PROPOSE_TOOL}/{correlation_id}"),
            source_excerpt_hash: None,
            run_id: None,
            supersedes: None,
            created_by_actor_id: self.service.actor_id().await?,
            correlation_id,
        };
        self.service
            .admit_and_store(candidate, &arguments.entity_ids, now)
            .await
    }
}

/// The arguments the model may send, after parsing.
#[derive(Clone, Debug)]
struct ProposalArguments {
    content: String,
    memory_type: MemoryType,
    entity_ids: Vec<String>,
    importance: u8,
}

impl ProposalArguments {
    /// Reads the arguments, refusing anything the schema should have prevented.
    ///
    /// The schema is checked by the pipeline before this runs, so a failure here means either a caller that
    /// bypassed validation or a schema the parser and the validator read differently. Both are faults worth
    /// reporting precisely rather than coercing: a defaulted `memory_type` would file a claim under a type
    /// nobody chose, and a dropped `entity_ids` entry would attach the claim to fewer subjects than the model
    /// named.
    fn parse(arguments: &serde_json::Value) -> Result<Self, String> {
        let content = arguments
            .get("content")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "the content argument is required and must be a string".to_owned())?
            .trim()
            .to_owned();
        if content.is_empty() {
            return Err("the content argument must not be blank".to_owned());
        }
        let memory_type = arguments
            .get("memory_type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "the memory_type argument is required".to_owned())?
            .parse::<MemoryType>()
            .map_err(|_| "the memory_type argument is not a type this build knows".to_owned())?;
        // Absent means "about the user": the service resolves an empty list to the profile owner's entity.
        let entity_ids = match arguments.get("entity_ids") {
            None => Vec::new(),
            Some(value) => {
                let named = value
                    .as_array()
                    .ok_or_else(|| "the entity_ids argument must be an array".to_owned())?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| "every entity_ids entry must be a string".to_owned())
                    })
                    .collect::<Result<Vec<String>, String>>()?;
                // Present but empty is malformed, not "the user": omitting the field is how that is asked for.
                if named.is_empty() {
                    return Err(
                        "the entity_ids argument must name at least one subject, or be left out"
                            .to_owned(),
                    );
                }
                named
            }
        };
        let importance = match arguments.get("importance") {
            None => crate::memory_service::DEFAULT_MEMORY_IMPORTANCE,
            Some(value) => {
                let value = value
                    .as_u64()
                    .ok_or_else(|| "the importance argument must be an integer".to_owned())?;
                u8::try_from(value)
                    .map_err(|_| "the importance argument is out of range".to_owned())?
            }
        };
        Ok(Self {
            content,
            memory_type,
            entity_ids,
            importance,
        })
    }
}

/// Names the field or kind a request-shaped refusal was about.
///
/// The four variants this covers are unreachable from the paths this adapter takes — it never pages, never
/// looks a claim up by identifier, and never writes against a version. Naming them anyway keeps the match
/// exhaustive, so a variant added later forces a decision here rather than falling into a wildcard that would
/// silently report it as something else.
fn field_or_kind(error: &MemoryServiceError) -> &'static str {
    match error {
        MemoryServiceError::UnknownValue { field, .. } => field,
        _ => "the request",
    }
}

/// Reports a stored candidate, or a refusal, as a bounded JSON outcome.
///
/// The `outcome` string is the service's own name for what happened, so this adapter does not introduce a
/// second vocabulary for the same fact — a translation layer here would be one more place the two could
/// disagree about whether a claim was stored.
fn confirm(
    outcome: &str,
    memory_id: &str,
    now: UtcTimestamp,
) -> Result<ToolCallResult, AdapterError> {
    let body = serde_json::json!({
        "outcome": outcome,
        "status": "proposed",
        "memory_id": memory_id,
    })
    .to_string();
    let evidence = ProviderEvidence::new(format!("memory:{outcome}")).map_err(|_| {
        AdapterError::ProviderRefused {
            reason: "the outcome was unusable as evidence".to_owned(),
        }
    })?;
    let record = ToolOutcomeRecord::confirmed(evidence.as_str()).map_err(|_| {
        AdapterError::ProviderRefused {
            reason: "the outcome was unusable as evidence".to_owned(),
        }
    })?;
    Ok(ToolCallResult::new(
        record,
        Some(evidence),
        Some(BoundedOutput::from_bounded(body, false)),
        now,
    ))
}

/// Reports a pipeline refusal as a completed call that stored nothing.
fn refusal(reason: &str, detail: &str, now: UtcTimestamp) -> ToolCallResult {
    let body = serde_json::json!({
        "outcome": "refused",
        "status": "none",
        "detail": detail,
    })
    .to_string();
    // The record is built from the **pipeline's own reason name**, so the outcome and its justification
    // come from one place. A `failed` record would be the wrong vocabulary: the candidate was judged, not
    // lost — and its reason is a stable identifier rather than the human-readable detail, which is the
    // distinction `ToolOutcomeRecord` draws between a diagnostic and a locator.
    let record = ToolOutcomeRecord::failed(reason).unwrap_or_else(|_| {
        ToolOutcomeRecord::failed("refused").unwrap_or_else(|_| unreachable_record())
    });
    ToolCallResult::new(
        record,
        None,
        Some(BoundedOutput::from_bounded(body, false)),
        now,
    )
}

#[cfg(test)]
#[path = "memory_propose_tests.rs"]
mod tests;

/// A record that cannot fail to build, for the case above where a literal was already refused.
///
/// Reachable only if the constant `"refused"` stopped satisfying the record's own bounds, which no change to
/// this file could cause. Panicking would turn a diagnostic into a crash; this reports the platform as the
/// source of the problem, which is what it would be.
fn unreachable_record() -> ToolOutcomeRecord {
    ToolOutcomeRecord::failed("the refusal could not be recorded").unwrap_or_else(|_| {
        // A `failed` record with a literal under every documented bound. If this is reached, the bounds moved
        // rather than the value, and a diagnostic naming that is more useful than a panic in an adapter.
        unreachable!("ToolOutcomeRecord::failed refused a bounded literal")
    })
}
