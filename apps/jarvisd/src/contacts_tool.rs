//! The tools a **model** keeps its contact list with: `jarvis.contacts.save`, `search` and `stats` (`ADR-0155`).
//!
//! A prospecting project that keeps its leads in a spreadsheet loses them whenever the file is rewritten, and a run cannot ask "who have we
//! already contacted?". These tools read and write the local contact list; nothing here sends anything to anyone.
//!
//! # Why they do not ask
//!
//! Saving a contact is writing a row in the owner's own database, the same class of effect as a project journal entry, and it is only data:
//! a later run reads it back fenced as data, never as an instruction. The one rule that matters is enforced in storage rather than in a
//! prompt: a model cannot move a contact out of `do_not_contact`, because that is the owner's word.

use std::sync::Arc;

use async_trait::async_trait;
use jarvis_core::{IsolatedText, Sensitivity, SystemClock, UtcTimestamp};
use jarvis_storage::{ContactInput, ContactStatus, DatabaseError, SqliteDatabase, StoredContact};
use jarvis_tools::{
    AdapterError, ApprovalPolicy, Availability, BoundedOutput, EffectSet, Idempotency,
    ProviderEvidence, RetryDeclaration, SchemaError, Scope, ScopeError, ScopeSet, ToolCallResult,
    ToolDefinition, ToolDefinitionError, ToolDefinitionParts, ToolEffect, ToolExecutionRequest,
    ToolExecutor, ToolId, ToolIdError, ToolOutcomeRecord, ToolSchema, ToolSensitivity, ToolSource,
};
use serde_json::{Value, json};
use thiserror::Error;

use crate::project_context::site_of_call;

/// Saves or updates one contact.
pub const SAVE_TOOL: &str = "jarvis.contacts.save";
/// Searches the contacts.
pub const SEARCH_TOOL: &str = "jarvis.contacts.search";
/// Counts the contacts by status.
pub const STATS_TOOL: &str = "jarvis.contacts.stats";
/// The scope a caller must hold to write contacts.
pub const WRITE_SCOPE: &str = "contacts.write";
/// The scope a caller must hold to read contacts.
pub const READ_SCOPE: &str = "contacts.read";

const SAVE_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "company": { "type": "string", "maxLength": 200, "description": "The company. Required for a new contact." },
    "person": { "type": "string", "maxLength": 200 },
    "role": { "type": "string", "maxLength": 200 },
    "email": { "type": "string", "maxLength": 254, "description": "Matches an existing contact by address, so saving the same lead again updates it." },
    "phone": { "type": "string", "maxLength": 40 },
    "status": { "type": "string", "enum": ["new", "contacted", "replied", "meeting", "won", "lost", "do_not_contact"], "description": "Where they stand. You cannot move a contact out of do_not_contact: that is the owner's word." },
    "notes": { "type": "string", "maxLength": 2000, "description": "Replaces the old notes, so read them first (jarvis.contacts.search) and keep what matters." },
    "source": { "type": "string", "maxLength": 300, "description": "Where the lead came from: a page, a list, a person." }
  }
}"#;

const SEARCH_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "query": { "type": "string", "maxLength": 200, "description": "Text to find in the company, person, address or notes. Omit to list." },
    "status": { "type": "string", "enum": ["new", "contacted", "replied", "meeting", "won", "lost", "do_not_contact"] },
    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "How many to return (20 by default)." }
  }
}"#;

const STATS_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "properties": {}
}"#;

const OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object"
}"#;

const TIMEOUT_SECONDS: u32 = 15;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;

/// Why a tool's own contract could not be built.
#[derive(Debug, Error)]
pub enum ContactsToolError {
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

/// The adapter behind the contact tools.
pub struct ContactsTool {
    database: Arc<SqliteDatabase>,
}

struct Spec {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    input: &'static str,
    effect: ToolEffect,
    risk: u8,
    scope: &'static str,
}

impl ContactsTool {
    /// Builds the adapter over the daemon's database.
    #[must_use]
    pub const fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// The tools' definitions.
    ///
    /// # Errors
    ///
    /// Returns [`ContactsToolError`] when a constant of a contract is rejected: a configuration fault, so the daemon fails at startup
    /// rather than on the first call.
    pub fn definitions() -> Result<Vec<ToolDefinition>, ContactsToolError> {
        let specs = [
            Spec {
                id: SAVE_TOOL,
                title: "Save a contact",
                description: "Saves a lead, customer or other contact in the local contact list, or updates it when the address (or company and person) \
                              is already there. Use it for every lead you find or contact, with where it stands, so the next run does not repeat the work. \
                              It sends nothing to anyone. A contact the owner marked do_not_contact stays that way.",
                input: SAVE_INPUT,
                effect: ToolEffect::Write,
                risk: 1,
                scope: WRITE_SCOPE,
            },
            Spec {
                id: SEARCH_TOOL,
                title: "Search contacts",
                description: "Finds contacts by text and status, most useful before approaching anyone: check whether they are already known, what was \
                              said, and whether they are do_not_contact (never approach those).",
                input: SEARCH_INPUT,
                effect: ToolEffect::ReadOnly,
                risk: 0,
                scope: READ_SCOPE,
            },
            Spec {
                id: STATS_TOOL,
                title: "Count contacts by status",
                description: "How many contacts stand at each status, to report progress.",
                input: STATS_INPUT,
                effect: ToolEffect::ReadOnly,
                risk: 0,
                scope: READ_SCOPE,
            },
        ];
        specs.iter().map(Self::definition).collect()
    }

    fn definition(spec: &Spec) -> Result<ToolDefinition, ContactsToolError> {
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(spec.id)?,
            version: "1.0.0".to_owned(),
            title: spec.title.to_owned(),
            description: spec.description.to_owned(),
            input_schema: ToolSchema::parse(spec.input)?,
            output_schema: ToolSchema::parse(OUTPUT)?,
            effects: EffectSet::single(spec.effect),
            risk: spec.risk,
            required_scopes: ScopeSet::single(Scope::new(spec.scope)?),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: TIMEOUT_SECONDS,
            // A retry would write twice.
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

    async fn save(
        &self,
        call_id: &str,
        arguments: &Value,
        now: UtcTimestamp,
    ) -> Result<Value, AdapterError> {
        let text = |name: &str| {
            arguments
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let status = match arguments.get("status").and_then(Value::as_str) {
            Some(name) => Some(
                ContactStatus::parse(name)
                    .ok_or_else(|| refused("that status is not one of the known ones"))?,
            ),
            None => None,
        };
        let input = ContactInput {
            company: text("company"),
            person: text("person"),
            role: text("role"),
            email: text("email"),
            phone: text("phone"),
            status,
            notes: text("notes"),
            source: text("source"),
        };
        let workspace = self.workspace().await?;
        // A contact found inside a project is filed under it, so a project's leads can be told apart.
        let project = site_of_call(&self.database, call_id)
            .await
            .and_then(|site| site.project)
            .map(|project| project.id);
        let (contact, created) = jarvis_storage::save_contact(
            &self.database,
            &workspace,
            project.as_deref(),
            &input,
            false,
            now,
        )
        .await
        .map_err(|error| storage_refusal(&error))?;
        Ok(
            json!({ "saved": if created { "created" } else { "updated" }, "contact": describe(&contact) }),
        )
    }

    async fn search(&self, arguments: &Value) -> Result<Value, AdapterError> {
        let status = match arguments.get("status").and_then(Value::as_str) {
            Some(name) => Some(
                ContactStatus::parse(name)
                    .ok_or_else(|| refused("that status is not one of the known ones"))?,
            ),
            None => None,
        };
        let limit = arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_LIMIT, |limit| {
                u32::try_from(limit).unwrap_or(MAX_LIMIT)
            })
            .clamp(1, MAX_LIMIT);
        let workspace = self.workspace().await?;
        let found = jarvis_storage::search_contacts(
            &self.database,
            &workspace,
            arguments.get("query").and_then(Value::as_str),
            status,
            limit,
        )
        .await
        .map_err(|_| refused("the contacts could not be read"))?;
        Ok(
            json!({ "count": found.len(), "contacts": found.iter().map(describe).collect::<Vec<_>>() }),
        )
    }

    async fn stats(&self) -> Result<Value, AdapterError> {
        let workspace = self.workspace().await?;
        let counts = jarvis_storage::contact_stats(&self.database, &workspace)
            .await
            .map_err(|_| refused("the contacts could not be counted"))?;
        let total: u32 = counts.iter().map(|(_, held)| held).sum();
        let by_status: serde_json::Map<String, Value> = counts
            .into_iter()
            .map(|(status, held)| (status.as_str().to_owned(), json!(held)))
            .collect();
        Ok(json!({ "total": total, "by_status": by_status }))
    }
}

/// One contact as the model reads it. Notes and source are text a page or a person may have supplied, so they are fenced as data.
fn describe(contact: &StoredContact) -> Value {
    let fenced = |text: &str| {
        (!text.is_empty())
            .then(|| {
                IsolatedText::new(text)
                    .ok()
                    .map(|isolated| isolated.render())
            })
            .flatten()
    };
    json!({
        "company": contact.company,
        "person": contact.person,
        "role": contact.role,
        "email": contact.email,
        "phone": contact.phone,
        "status": contact.status.as_str(),
        "notes": fenced(&contact.notes),
        "source": fenced(&contact.source),
        "updated_at": contact.updated_at,
    })
}

fn storage_refusal(error: &DatabaseError) -> AdapterError {
    match error {
        DatabaseError::InvalidContact { field: "status" } => {
            refused("this contact is do_not_contact, and only the owner can change that")
        }
        DatabaseError::InvalidContact { field: "company" } => {
            refused("a new contact needs a company")
        }
        DatabaseError::InvalidContact { field } => refused(&format!(
            "the contact's {field} is not acceptable (too long, or not valid)"
        )),
        DatabaseError::ContactLimit => refused("the contact list is full"),
        _ => refused("the contact could not be saved"),
    }
}

fn refused(reason: &str) -> AdapterError {
    AdapterError::RefusedBeforeReaching {
        reason: reason.to_owned(),
    }
}

impl std::fmt::Debug for ContactsTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContactsTool")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ToolExecutor for ContactsTool {
    fn adapter_id(&self) -> &'static str {
        "contacts"
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
        let (call, arguments) = (request.call_id(), request.arguments());
        let body = match tool.as_str() {
            SAVE_TOOL => self.save(call, arguments, now).await?,
            SEARCH_TOOL => self.search(arguments).await?,
            STATS_TOOL => self.stats().await?,
            _ => return Err(AdapterError::NotImplemented { tool }),
        };
        let evidence = ProviderEvidence::new("contacts:store").ok();
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
#[path = "contacts_tool_tests.rs"]
mod tests;
