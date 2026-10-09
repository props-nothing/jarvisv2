//! The tool a **model** keeps a project's journal with: `jarvis.project.note`.
//!
//! A project outlives any one run (`ADR-0151`). What lets tomorrow's run continue today's rather than restart it is a short written
//! record, so the model is asked to leave one when it finishes a stage, decides something, or is blocked, and the next run is told the
//! latest entries.
//!
//! # The project comes from the call, never from the arguments
//!
//! The tool has no project argument. It looks up the call, the run that made it, and that run's conversation, and writes to the
//! project that conversation belongs to. A model therefore cannot write into a project it is not working in, and a conversation that
//! is in no project is told so rather than given somewhere to write.
//!
//! # Why this asks no approval
//!
//! An entry is one short piece of text in the owner's own journal. It reaches no outside system and changes no file, and the owner
//! can read and delete the whole project. Asking for each one would bury the approvals that matter, which is the opposite of keeping
//! approvals light (`ADR-0136`). A model-written entry is shown to later runs as fenced data, never as an instruction.

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{Sensitivity, SystemClock, UtcTimestamp};
use jarvis_storage::{DatabaseError, NoteKind, SqliteDatabase};
use jarvis_tools::{
    AdapterError, ApprovalPolicy, Availability, BoundedOutput, EffectSet, Idempotency,
    ProviderEvidence, RetryDeclaration, SchemaError, Scope, ScopeError, ScopeSet, ToolCallResult,
    ToolDefinition, ToolDefinitionError, ToolDefinitionParts, ToolEffect, ToolExecutionRequest,
    ToolExecutor, ToolId, ToolIdError, ToolOutcomeRecord, ToolSchema, ToolSensitivity, ToolSource,
};
use serde_json::{Value, json};
use thiserror::Error;

/// Records one journal entry in the project this conversation belongs to.
pub const NOTE_TOOL: &str = "jarvis.project.note";
/// The scope a caller must hold for the note tool.
pub const NOTE_SCOPE: &str = "project.note";

const INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["kind", "text"],
  "properties": {
    "kind": { "type": "string", "enum": ["progress", "decision", "blocker", "next", "result"], "description": "progress: something was done. decision: a choice was made, and why. blocker: something stops the work. next: what should happen next. result: a finished deliverable or finding." },
    "text": { "type": "string", "minLength": 1, "maxLength": 4000, "description": "One or two plain sentences that the next run can act on without this conversation: what, where it is (a file path, a name), and why it matters." }
  }
}"#;

const OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object"
}"#;

const TIMEOUT_SECONDS: u32 = 15;

/// Why the note tool's own contract could not be built.
#[derive(Debug, Error)]
pub enum ProjectToolError {
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

/// The adapter behind `jarvis.project.note`.
pub struct ProjectTool {
    database: Arc<SqliteDatabase>,
}

impl ProjectTool {
    /// Builds the adapter over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// The tool's definition.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectToolError`] when a constant of the contract is rejected: a configuration fault, so the daemon fails at
    /// startup rather than on the first call.
    pub fn definition() -> Result<ToolDefinition, ProjectToolError> {
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(NOTE_TOOL)?,
            version: "1.0.0".to_owned(),
            title: "Write a project journal entry".to_owned(),
            description: "Records a short entry in the journal of the project this conversation belongs to, so the next run \
                          continues instead of restarting. Use it when you finish a stage, decide something, are blocked, or know \
                          what should happen next. It only works inside a project."
                .to_owned(),
            input_schema: ToolSchema::parse(INPUT)?,
            output_schema: ToolSchema::parse(OUTPUT)?,
            effects: EffectSet::single(ToolEffect::Write),
            risk: 1,
            required_scopes: ScopeSet::single(Scope::new(NOTE_SCOPE)?),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: TIMEOUT_SECONDS,
            // A retry would write the entry twice.
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Unsupported,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })?)
    }

    async fn note(
        &self,
        call_id: &str,
        arguments: &Value,
        now: UtcTimestamp,
    ) -> Result<Value, AdapterError> {
        let kind = arguments
            .get("kind")
            .and_then(Value::as_str)
            .and_then(NoteKind::parse)
            // The owner's own kind is not the model's to claim.
            .filter(|kind| *kind != NoteKind::Owner)
            .ok_or_else(|| {
                refused("the kind must be progress, decision, blocker, next or result")
            })?;
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("a text is required"))?;
        let Some((project, run_id)) =
            crate::project_context::project_of_call(&self.database, call_id).await
        else {
            return Err(refused(
                "this conversation is not part of a project, so there is no journal to write to",
            ));
        };
        jarvis_storage::add_project_note(
            &self.database,
            &project.id,
            kind,
            text,
            Some(&run_id),
            now,
        )
        .await
        .map_err(|error| match error {
            DatabaseError::InvalidProject { .. } => {
                refused("the text must be 1 to 4000 characters")
            }
            _ => refused("the entry could not be saved"),
        })?;
        Ok(json!({ "recorded": kind.as_str(), "project": project.name }))
    }
}

fn refused(reason: &str) -> AdapterError {
    AdapterError::RefusedBeforeReaching {
        reason: reason.to_owned(),
    }
}

impl std::fmt::Debug for ProjectTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectTool")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolExecutor for ProjectTool {
    fn adapter_id(&self) -> &'static str {
        "project"
    }

    async fn execute(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        let tool = request.tool().to_string();
        if tool != NOTE_TOOL {
            return Err(AdapterError::NotImplemented { tool });
        }
        let now = UtcTimestamp::now(&SystemClock);
        if request.is_past_deadline(now) {
            return Err(refused("the call was already past its deadline"));
        }
        let body = self
            .note(request.call_id(), request.arguments(), now)
            .await?;
        let evidence = ProviderEvidence::new("project:journal").ok();
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
#[path = "project_tool_tests.rs"]
mod tests;
