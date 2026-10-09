//! The tools a **model** corrects and forgets memories with: `jarvis.memory.correct` and `jarvis.memory.forget`.
//!
//! Memory is the owner's. A model could already *propose* a claim (`jarvis.memory.propose`, which waits for the owner to keep it) and look
//! things up (`jarvis.memory.search`); it could not repair what was wrong, so a stale fact stayed until the owner opened a terminal. These
//! two close that, under the same rule as every effect that outlives a conversation: the owner is asked, once.
//!
//! # The owner is shown which claim, and a lie fails closed
//!
//! An approval card shows a call's arguments, and a memory id means nothing to a person. So the call carries a `claim`: a quotation of the
//! memory it is about. After the owner says yes, **the tool checks the quotation against the stored claim** and refuses when it is not
//! there, so a model cannot show one claim and change another: what the owner read is what is changed, or nothing is. The write itself
//! goes through the same service as the HTTP surface (version-guarded, tombstoned on forget), so no second path decides what a correction
//! may be.

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{Sensitivity, SystemClock, UtcTimestamp};
use jarvis_protocol::{CorrectMemoryRequest, ForgetMemoryRequest};
use jarvis_storage::SqliteDatabase;
use jarvis_tools::{
    AdapterError, ApprovalPolicy, Availability, BoundedOutput, EffectSet, Idempotency,
    ProviderEvidence, RetryDeclaration, SchemaError, Scope, ScopeError, ScopeSet, ToolCallResult,
    ToolDefinition, ToolDefinitionError, ToolDefinitionParts, ToolEffect, ToolExecutionRequest,
    ToolExecutor, ToolId, ToolIdError, ToolOutcomeRecord, ToolSchema, ToolSensitivity, ToolSource,
};
use serde_json::{Value, json};
use thiserror::Error;

use crate::memory_service::{MemoryService, MemoryServiceError};

/// Replaces a remembered claim with a corrected one.
pub const CORRECT_TOOL: &str = "jarvis.memory.correct";
/// Deletes a remembered claim.
pub const FORGET_TOOL: &str = "jarvis.memory.forget";
/// The scope a caller must hold for these tools.
pub const MANAGE_SCOPE: &str = "memory.manage";

/// The shortest quotation accepted, so a single common word cannot stand for a claim.
const MIN_QUOTE_CHARS: usize = 8;

const CORRECT_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["memory_id", "claim", "corrected"],
  "properties": {
    "memory_id": { "type": "string", "minLength": 36, "maxLength": 36, "description": "The memory_id jarvis.memory.search gave." },
    "claim": { "type": "string", "minLength": 1, "maxLength": 300, "description": "A quotation from the memory being corrected, at least 8 characters (or all of it if shorter). The owner reads this to know which memory; the tool refuses if it is not in that memory." },
    "corrected": { "type": "string", "minLength": 1, "maxLength": 2000, "description": "The corrected claim, complete: it replaces the old one." }
  }
}"#;

const FORGET_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["memory_id", "claim"],
  "properties": {
    "memory_id": { "type": "string", "minLength": 36, "maxLength": 36, "description": "The memory_id jarvis.memory.search gave." },
    "claim": { "type": "string", "minLength": 1, "maxLength": 300, "description": "A quotation from the memory to delete, at least 8 characters (or all of it if shorter). The owner reads this to know which memory; the tool refuses if it is not in that memory." }
  }
}"#;

const OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object"
}"#;

const TIMEOUT_SECONDS: u32 = 30;

/// Why the tools' own contract could not be built.
#[derive(Debug, Error)]
pub enum MemoryManageToolError {
    /// A tool identifier constant was rejected.
    #[error(transparent)]
    Id(#[from] ToolIdError),
    /// A schema constant was rejected.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// A scope constant was rejected.
    #[error(transparent)]
    Scope(#[from] ScopeError),
    /// A definition was rejected.
    #[error(transparent)]
    Definition(#[from] ToolDefinitionError),
}

/// The adapter behind the two tools.
pub struct MemoryManageTool {
    service: MemoryService,
}

impl MemoryManageTool {
    /// Builds the adapter over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self {
            service: MemoryService::new(database),
        }
    }

    /// The two definitions.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryManageToolError`] when a constant of a contract is rejected: a configuration fault.
    pub fn definitions() -> Result<Vec<ToolDefinition>, MemoryManageToolError> {
        Ok(vec![
            Self::definition(
                CORRECT_TOOL,
                "Correct a memory",
                "Replaces a remembered claim that is wrong or out of date with a corrected one (memory_id from jarvis.memory.search). Quote the \
                 claim being replaced so the owner can see which one. The owner is asked first.",
                CORRECT_INPUT,
            )?,
            Self::definition(
                FORGET_TOOL,
                "Forget a memory",
                "Deletes a remembered claim for good (memory_id from jarvis.memory.search), so it is not remembered again unless the owner says it \
                 anew. Quote the claim so the owner can see which one. The owner is asked first.",
                FORGET_INPUT,
            )?,
        ])
    }

    fn definition(
        id: &str,
        title: &str,
        description: &str,
        input: &str,
    ) -> Result<ToolDefinition, MemoryManageToolError> {
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(id)?,
            version: "1.0.0".to_owned(),
            title: title.to_owned(),
            description: description.to_owned(),
            input_schema: ToolSchema::parse(input)?,
            output_schema: ToolSchema::parse(OUTPUT)?,
            effects: EffectSet::single(ToolEffect::Write),
            risk: 2,
            required_scopes: ScopeSet::single(Scope::new(MANAGE_SCOPE)?),
            approval: ApprovalPolicy::Ask,
            timeout_seconds: TIMEOUT_SECONDS,
            // A retry of a deletion or a correction is a second one.
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Unsupported,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })?)
    }

    /// Reads the memory and checks the quotation the owner approved is really in it. Returns its version.
    async fn verified_version(&self, arguments: &Value) -> Result<(String, i64), AdapterError> {
        let id = text(arguments, "memory_id").ok_or_else(|| refused("a memory_id is required"))?;
        let claim =
            text(arguments, "claim").ok_or_else(|| refused("a claim quotation is required"))?;
        let detail = self
            .service
            .read(id)
            .await
            .map_err(|error| service_refusal(&error))?;
        if !quotation_matches(claim, &detail.content) {
            return Err(refused(
                "the quoted claim is not in that memory, so nothing was changed; search again and quote the memory you mean",
            ));
        }
        Ok((id.to_owned(), detail.reference.version))
    }

    async fn forget(&self, arguments: &Value) -> Result<Value, AdapterError> {
        let (id, version) = self.verified_version(arguments).await?;
        let receipt = self
            .service
            .forget(
                &id,
                &ForgetMemoryRequest {
                    expected_version: version,
                    allow_relearn: false,
                },
            )
            .await
            .map_err(|error| service_refusal(&error))?;
        Ok(
            json!({ "forgotten": id, "tombstone_written": receipt.tombstone_written, "caveats": receipt.unreachable }),
        )
    }

    async fn correct(&self, arguments: &Value) -> Result<Value, AdapterError> {
        let corrected = text(arguments, "corrected")
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| refused("the corrected claim is required"))?;
        let (id, version) = self.verified_version(arguments).await?;
        let reply = self
            .service
            .correct(
                &id,
                &CorrectMemoryRequest {
                    content: corrected.to_owned(),
                    expected_version: version,
                    entity_ids: None,
                },
            )
            .await
            .map_err(|error| service_refusal(&error))?;
        Ok(json!({ "corrected": id, "replaced_by": reply.memory_id }))
    }
}

fn text<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

/// Whether the quotation is really in the claim: case and spacing ignored, long enough to mean something (or the whole claim).
fn quotation_matches(quotation: &str, content: &str) -> bool {
    let flat = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let (quote, whole) = (flat(quotation), flat(content));
    if quote.is_empty() {
        return false;
    }
    if quote.chars().count() < MIN_QUOTE_CHARS && quote != whole {
        return false;
    }
    whole.contains(&quote)
}

fn service_refusal(error: &MemoryServiceError) -> AdapterError {
    match error {
        MemoryServiceError::NotFound => refused("no memory has that id"),
        MemoryServiceError::Conflict => refused("the memory changed meanwhile; search again"),
        MemoryServiceError::Refused { detail, .. } => refused(detail),
        _ => refused("the memory could not be changed"),
    }
}

fn refused(reason: &str) -> AdapterError {
    AdapterError::RefusedBeforeReaching {
        reason: reason.to_owned(),
    }
}

impl std::fmt::Debug for MemoryManageTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryManageTool")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolExecutor for MemoryManageTool {
    fn adapter_id(&self) -> &'static str {
        "memory-manage"
    }

    async fn execute(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        let tool = request.tool().to_string();
        let now = UtcTimestamp::now(&SystemClock);
        if request.is_past_deadline(now) {
            return Err(refused("the call was already past its deadline"));
        }
        let body = match tool.as_str() {
            CORRECT_TOOL => self.correct(request.arguments()).await?,
            FORGET_TOOL => self.forget(request.arguments()).await?,
            _ => return Err(AdapterError::NotImplemented { tool }),
        };
        let evidence = ProviderEvidence::new("memory:manage").ok();
        let record = evidence
            .as_ref()
            .and_then(|value| ToolOutcomeRecord::confirmed(value.as_str()).ok())
            .unwrap_or_else(|| {
                ToolOutcomeRecord::failed("evidence")
                    .unwrap_or_else(|_| unreachable!("a literal reason"))
            });
        Ok(ToolCallResult::new(
            record,
            evidence,
            Some(BoundedOutput::from_bounded(body.to_string(), false)),
            now,
        ))
    }
}

#[cfg(test)]
#[path = "memory_manage_tests.rs"]
mod tests;
