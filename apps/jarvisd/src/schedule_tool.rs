//! The tools a **model** schedules work through: `jarvis.schedule.add`, `jarvis.schedule.list` and `jarvis.schedule.remove`.
//!
//! "Remind me tomorrow at nine", "check this every morning". The scheduler already ran tasks that fire while the owner is away
//! (`jarvis schedule`); this puts it in reach of a conversation.
//!
//! # What a schedule is allowed to be
//!
//! A scheduled task is an ordinary run started later, through the same executor, policy and approvals, so it can do nothing
//! the owner could not have asked for now, and anything that needs an approval parks for the owner as any run does. The one
//! thing that is new is **persistence**: a recurring task outlives the conversation that made it. A web page a model read
//! could therefore try to leave a standing instruction behind, which is why adding and removing a schedule **ask the owner
//! first** (one click, and "Always allow" is on the card for someone who wants none), while listing is free.
//!
//! # What the model cannot say
//!
//! Cadence limits, the per-workspace cap and the objective's length are the scheduler's own, enforced by the same functions the
//! API and the CLI use ([`validate_objective`], [`parse_interval`], [`Cadence`]); this adapter adds no rule of its own and
//! reports a refusal in the scheduler's words.

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{
    Cadence, InvalidSchedule, IsolatedText, Sensitivity, SystemClock, UtcTimestamp, parse_interval,
    validate_objective,
};
use jarvis_storage::{DatabaseError, SqliteDatabase, StoredSchedule};
use jarvis_tools::{
    AdapterError, ApprovalPolicy, Availability, BoundedOutput, EffectSet, Idempotency,
    ProviderEvidence, RetryDeclaration, SchemaError, Scope, ScopeError, ScopeSet, ToolCallResult,
    ToolDefinition, ToolDefinitionError, ToolDefinitionParts, ToolEffect, ToolExecutionRequest,
    ToolExecutor, ToolId, ToolIdError, ToolOutcomeRecord, ToolSchema, ToolSensitivity, ToolSource,
};
use serde_json::{Value, json};
use thiserror::Error;

/// Creates a scheduled task.
pub const ADD_TOOL: &str = "jarvis.schedule.add";
/// Lists the scheduled tasks.
pub const LIST_TOOL: &str = "jarvis.schedule.list";
/// Removes a scheduled task.
pub const REMOVE_TOOL: &str = "jarvis.schedule.remove";
/// The scope a caller must hold for the schedule tools.
pub const SCHEDULE_SCOPE: &str = "schedule.manage";

const ADD_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["objective"],
  "properties": {
    "objective": { "type": "string", "minLength": 1, "maxLength": 2000, "description": "What to do each time it fires, written as the instruction you would be given then, complete on its own: it runs later with no memory of this chat." },
    "every": { "type": "string", "pattern": "^[0-9]{1,6}[mhd]$", "description": "Repeat on this interval: 30m, 6h or 2d. At least 1 minute. Give this or `at`, not both." },
    "at": { "type": "string", "description": "Run once at this UTC time, RFC 3339, for example 2026-10-10T07:00:00Z. Must be in the future. Work it out from the current date and time you were given, converting from the user's local time. Give this or `every`, not both." }
  }
}"#;

const LIST_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {}
}"#;

const REMOVE_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["schedule_id"],
  "properties": {
    "schedule_id": { "type": "string", "minLength": 1, "maxLength": 64, "description": "The id from jarvis.schedule.list." }
  }
}"#;

const OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object"
}"#;

const TIMEOUT_SECONDS: u32 = 30;

/// Why the schedule tools' own contract could not be built.
#[derive(Debug, Error)]
pub enum ScheduleToolError {
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

/// The adapter behind the three schedule tools.
pub struct ScheduleTool {
    database: Arc<SqliteDatabase>,
}

impl ScheduleTool {
    /// Builds the adapter over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// The three definitions.
    ///
    /// # Errors
    ///
    /// Returns [`ScheduleToolError`] when a constant of a contract is rejected: a configuration fault, so the daemon fails at
    /// startup rather than on the first call.
    pub fn definitions() -> Result<Vec<ToolDefinition>, ScheduleToolError> {
        Ok(vec![
            Self::definition(
                ADD_TOOL,
                "Schedule a task",
                "Schedules a task to run later or repeatedly, for example a reminder tomorrow at nine or a check every \
                 morning, using `at` (one UTC time) or `every` (30m, 6h, 2d). The task runs as an ordinary run with the same \
                 tools and approval rules, with no memory of this conversation, so write the objective complete. The user is \
                 asked first.",
                ADD_INPUT,
                ToolEffect::Write,
                2,
                ApprovalPolicy::Ask,
            )?,
            Self::definition(
                LIST_TOOL,
                "List scheduled tasks",
                "Lists the scheduled tasks with their ids, objectives, cadence and next run.",
                LIST_INPUT,
                ToolEffect::ReadOnly,
                0,
                ApprovalPolicy::Auto,
            )?,
            Self::definition(
                REMOVE_TOOL,
                "Remove a scheduled task",
                "Removes one scheduled task by the id jarvis.schedule.list gave. The user is asked first.",
                REMOVE_INPUT,
                ToolEffect::Write,
                2,
                ApprovalPolicy::Ask,
            )?,
        ])
    }

    fn definition(
        id: &str,
        title: &str,
        description: &str,
        input: &str,
        effect: ToolEffect,
        risk: u8,
        approval: ApprovalPolicy,
    ) -> Result<ToolDefinition, ScheduleToolError> {
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(id)?,
            version: "1.0.0".to_owned(),
            title: title.to_owned(),
            description: description.to_owned(),
            input_schema: ToolSchema::parse(input)?,
            output_schema: ToolSchema::parse(OUTPUT)?,
            effects: EffectSet::single(effect),
            risk,
            required_scopes: ScopeSet::single(Scope::new(SCHEDULE_SCOPE)?),
            approval,
            timeout_seconds: TIMEOUT_SECONDS,
            // A retry of an add would create a second task.
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Unsupported,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })?)
    }

    async fn workspace(&self) -> Result<String, AdapterError> {
        jarvis_storage::load_local_identity(&self.database)
            .await
            .map(|identity| identity.workspace_id().to_owned())
            .map_err(|_| refused("the local database is not available"))
    }

    async fn add(&self, arguments: &Value, now: UtcTimestamp) -> Result<Value, AdapterError> {
        let objective = arguments
            .get("objective")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("an objective is required"))?;
        let objective =
            validate_objective(objective).map_err(|error| refused(&error.to_string()))?;
        let every = arguments.get("every").and_then(Value::as_str);
        let at = arguments.get("at").and_then(Value::as_str);
        let cadence = match (every, at) {
            (Some(every), None) => parse_interval(every).and_then(Cadence::every_seconds),
            (None, Some(at)) => {
                let at = at.parse::<UtcTimestamp>().map_err(|_| {
                    refused("`at` must be an RFC 3339 time such as 2026-10-10T07:00:00Z")
                })?;
                Cadence::once_at(at, now)
            }
            _ => Err(InvalidSchedule::ExactlyOneCadence),
        }
        .map_err(|error| refused(&error.to_string()))?;
        let workspace = self.workspace().await?;
        let stored =
            jarvis_storage::create_schedule(&self.database, &workspace, &objective, cadence, now)
                .await
                .map_err(|error| match error {
                    DatabaseError::ScheduleLimit => refused(
                        "this workspace already holds the maximum number of scheduled tasks",
                    ),
                    _ => refused("the schedule could not be saved"),
                })?;
        Ok(json!({ "scheduled": describe(&stored) }))
    }

    async fn list(&self) -> Result<Value, AdapterError> {
        let workspace = self.workspace().await?;
        let schedules = jarvis_storage::list_schedules(&self.database, &workspace)
            .await
            .map_err(|_| refused("the schedules could not be read"))?;
        Ok(json!({ "schedules": schedules.iter().map(describe).collect::<Vec<_>>() }))
    }

    async fn remove(&self, arguments: &Value) -> Result<Value, AdapterError> {
        let id = arguments
            .get("schedule_id")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("a schedule_id is required"))?;
        let workspace = self.workspace().await?;
        jarvis_storage::delete_schedule(&self.database, &workspace, id)
            .await
            .map_err(|error| match error {
                DatabaseError::ScheduleNotFound => refused("no scheduled task has that id"),
                _ => refused("the schedule could not be removed"),
            })?;
        Ok(json!({ "removed": id }))
    }
}

/// One schedule as the model reads it. The objective is text a model (or a page it read) once wrote, so it is fenced as data.
fn describe(schedule: &StoredSchedule) -> Value {
    let (cadence, interval) = match schedule.cadence() {
        Cadence::Every(seconds) => ("every", Some(seconds)),
        Cadence::Once(_) => ("once", None),
    };
    json!({
        "schedule_id": schedule.id(),
        "objective": IsolatedText::new(schedule.objective()).ok().map(|text| text.render()),
        "cadence": cadence,
        "interval_seconds": interval,
        "enabled": schedule.enabled(),
        "next_run_at": schedule.enabled().then(|| schedule.next_run().to_string()),
        "fire_count": schedule.fire_count(),
    })
}

fn refused(reason: &str) -> AdapterError {
    AdapterError::RefusedBeforeReaching {
        reason: reason.to_owned(),
    }
}

impl std::fmt::Debug for ScheduleTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScheduleTool")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolExecutor for ScheduleTool {
    fn adapter_id(&self) -> &'static str {
        "schedule"
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
            ADD_TOOL => self.add(request.arguments(), now).await?,
            LIST_TOOL => self.list().await?,
            REMOVE_TOOL => self.remove(request.arguments()).await?,
            _ => return Err(AdapterError::NotImplemented { tool }),
        };
        let evidence = ProviderEvidence::new("schedule:store").ok();
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
#[path = "schedule_tool_tests.rs"]
mod tests;
