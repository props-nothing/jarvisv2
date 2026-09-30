//! The Google operations as model-facing tool definitions.
//!
//! # Why this is derived rather than written out
//!
//! `P3-006d` records the rule and the reason: the tool pipeline "registers the adapter's OWN
//! `definitions()`, not a restated copy of schema+risk. Policy decides about a `ToolDefinition`, so a copy
//! would be policy deciding about a tool OTHER than the one being run." The manifest is this connector's
//! declaration of what its operations do, so a second hand-written list of tools would be a second answer to
//! the same question — and the two would disagree the first time either changed.
//!
//! So [`definitions`] reads the manifest and produces one [`ToolDefinition`] per operation. Every effect,
//! risk, scope and idempotency value comes from the manifest operation it belongs to; only the schemas, the
//! title, the timeout and the retry policy are stated here, because the manifest has no field for them.
//!
//! # The decisions this module makes, and why each is not a default
//!
//! - **The namespace is `google.<operation id>`**, so `google.gmail_messages_read` is what a policy decision
//!   names. [`ToolSource::from_namespace`] classifies a namespace by its **leading segment**, so the `google`
//!   prefix is what makes these `Connector` tools rather than `Native` ones — which is the whole reason a
//!   tool's source is derived from its identifier rather than declared.
//! - **`Idempotency` is derived from the manifest's `ProviderIdempotency`**, not chosen. The two are
//!   different vocabularies for one question ("is a repeat safe"), and the mapping is explicit and total so a
//!   new variant on either side is a compile error rather than a silent default.
//! - **Retries are permitted only where the manifest says a repeat is safe.** `RetryPolicy::blind` refuses a
//!   retry for a mutating effect whose repeats are not made safe; a read-only effect passes that check, so
//!   the check alone would let this module retry everything. It therefore also consults
//!   [`ProviderIdempotency::permits_automatic_retry`] and refuses to retry an operation whose provider
//!   behaviour is `Unknown` **or** `NotIdempotent`, because a blind retry is only free when repeating the
//!   call is free.
//! - **`timeout_seconds` is a bound this connector sets, not a Google figure.** Google documents no
//!   per-request deadline, so the value is recorded as a JARVIS choice rather than presented as a provider
//!   fact — the same distinction [`crate::ratelimit::RateLimitEvidence`] exists to keep.
//! - **The output classification is `Confidential` for mail and calendar content, and the input is
//!   `Internal`.** Both halves are required and they are genuinely different: what a caller *sends* is an
//!   identifier or a query, while what comes back is the user's mail. A single field would have to be the
//!   maximum, which would over-restrict the input and hide what the tool actually consumes — the reason
//!   `ToolSensitivity` carries two values.
//!
//! # What is not here
//!
//! No executor. These definitions describe tools nothing can run: there is no transport, no token source and
//! no operation implementation, so [`crate::google::GoogleConnector::manifest`]'s
//! `CompatibilityVerdict::Unverified` still stands and no model can reach a mailbox.

use jarvis_core::Sensitivity;
use jarvis_tools::{
    ApprovalPolicy, Availability, EffectSet, Idempotency, RetryDeclaration, Risk, SchemaError,
    Scope, ScopeError, ScopeSet, TOOL_SCHEMA_DIALECT, ToolDefinition, ToolDefinitionError,
    ToolDefinitionParts, ToolId, ToolIdError, ToolSchema, ToolSensitivity, ToolSource,
};

use crate::manifest::{ConnectorManifest, ProviderIdempotency, ValidatedOperation};

/// The behaviour version every Google tool starts at.
///
/// Separate from the connector's own version and from Google's API revision, because it is the version of
/// *this tool's contract* — its schemas and declared effects. `P3-002` requires a version change when
/// behaviour changes, because a stored intent naming a version must mean what it meant when it was written.
pub const TOOL_VERSION: &str = "1.0.0";

/// The execution deadline each Google tool is given, in seconds.
///
/// **A JARVIS choice, not a Google figure.** Google publishes no per-request deadline for Gmail or Calendar,
/// and the only timing facts the research record establishes are the quota windows and the batch ceiling. So
/// this is a bound this connector sets to keep a stalled read from holding a worker, and it is recorded as
/// such rather than presented as a provider limit.
pub const TOOL_TIMEOUT_SECONDS: u32 = 30;

/// The blind retries a read is permitted when the provider says a repeat is safe.
///
/// Two attempts, matching `P3-006`'s filesystem read tools, and permitted *only* when
/// [`ProviderIdempotency::permits_automatic_retry`] is true for the operation. A read does not change
/// anything, so repeating it costs a quota unit and nothing else — which is why the condition is about the
/// provider's behaviour rather than about safety.
pub const TOOL_RETRY_ATTEMPTS: u8 = 2;

/// The backoff ceiling for those retries, in seconds.
///
/// Google's own guidance is to start retrying "at least one second after the error" with
/// `maximum_backoff` "typically 32 or 64 seconds". This is the **ceiling** a tool's retry policy is declared
/// with, so it is the provider's lower figure — a tool that backed off for a full minute inside a request
/// with a 30-second deadline would time out rather than retry.
pub const TOOL_BACKOFF_CEILING_SECONDS: u32 = 32;

/// The input schema of one operation, as a JSON Schema 2020-12 document.
///
/// A table rather than a function of the operation's id, so adding an operation to the manifest **without**
/// adding its schema is a refusal rather than a tool with an invented contract. The alternative — deriving a
/// permissive schema from the id — would advertise a tool whose arguments are unconstrained, which is the
/// opposite of what a schema is for.
fn input_schema(operation: &str) -> Option<&'static str> {
    match operation {
        "gmail_messages_list" => Some(GMAIL_LIST_INPUT),
        "gmail_history_list" => Some(GMAIL_HISTORY_INPUT),
        "gmail_messages_read" => Some(GMAIL_READ_INPUT),
        "gmail_profile_read" => Some(GMAIL_PROFILE_INPUT),
        "calendar_events_read" => Some(CALENDAR_READ_INPUT),
        _ => None,
    }
}

/// The output schema of one operation.
fn output_schema(operation: &str) -> Option<&'static str> {
    match operation {
        "gmail_messages_list" => Some(GMAIL_LIST_OUTPUT),
        "gmail_history_list" => Some(GMAIL_HISTORY_OUTPUT),
        "gmail_messages_read" => Some(GMAIL_READ_OUTPUT),
        "gmail_profile_read" => Some(GMAIL_PROFILE_OUTPUT),
        "calendar_events_read" => Some(CALENDAR_READ_OUTPUT),
        _ => None,
    }
}

/// A short title for one operation.
fn title(operation: &str) -> Option<&'static str> {
    match operation {
        "gmail_messages_list" => Some("List Gmail messages"),
        "gmail_history_list" => Some("List Gmail changes since a position"),
        "gmail_messages_read" => Some("Read a Gmail message"),
        "gmail_profile_read" => Some("Identify the connected Gmail account"),
        "calendar_events_read" => Some("Read calendar events"),
        _ => None,
    }
}

/// The scope a caller must hold, which is also the manifest operation's `required_scopes` entry.
///
/// Taken from the manifest rather than restated, so a policy decision about the tool and the connector's own
/// account of what it needs cannot disagree. This function exists only to name the check.
fn caller_scopes(operation: &ValidatedOperation) -> Result<ScopeSet, ScopeError> {
    let mut scopes = Vec::with_capacity(operation.required_scopes().len());
    for scope in operation.required_scopes() {
        scopes.push(Scope::new(scope.clone())?);
    }
    Ok(ScopeSet::new(scopes))
}

/// Maps the manifest's provider idempotency onto the tool contract's idempotency.
///
/// **Explicit and total**, because these are two vocabularies for one question and a default would answer it
/// silently. The distinction that matters is between the two safe variants:
/// [`ProviderIdempotency::Declared`] means the provider makes a repeat a no-op, which is
/// [`Idempotency::ProviderKey`] in `jarvis-tools`' terms only when the caller must supply the key — so a
/// `Declared` repeat needs **no** JARVIS key while a `ProviderKey` one does. Mapping them the other way
/// round would either demand a key the provider ignores or omit one it requires.
#[must_use]
pub const fn tool_idempotency(declared: ProviderIdempotency) -> Idempotency {
    match declared {
        ProviderIdempotency::Declared => Idempotency::ProviderKey,
        ProviderIdempotency::ProviderKey => Idempotency::Required,
        // `Unknown` and `NotIdempotent` both become `Unsupported`, which is the honest answer: neither
        // claims a repeat is safe. They remain distinguishable in the manifest for a reader, and collapsing
        // them here is safe because `Idempotency::Unsupported` refuses repeats either way.
        ProviderIdempotency::Unknown | ProviderIdempotency::NotIdempotent => {
            Idempotency::Unsupported
        }
    }
}

/// Returns the retry declaration an operation's provider idempotency permits.
///
/// **A read passes `RetryPolicy::blind`'s check regardless of idempotency**, because that check asks whether
/// an effect is mutating. So this consults the manifest's own answer as well: a blind retry is only free when
/// repeating the call is free, and an operation whose provider behaviour is `Unknown` or `NotIdempotent` gets
/// no automatic retry however harmless its effect looks.
#[must_use]
pub const fn retry_declaration(declared: ProviderIdempotency) -> RetryDeclaration {
    if declared.permits_automatic_retry() {
        RetryDeclaration {
            attempts: TOOL_RETRY_ATTEMPTS,
            backoff_ceiling_seconds: TOOL_BACKOFF_CEILING_SECONDS,
        }
    } else {
        RetryDeclaration::none()
    }
}

/// Why a manifest operation could not be turned into a tool definition.
///
/// A separate error rather than a transparent wrap, because three of these are **manifest** defects that an
/// author fixes and one is a contract violation that points at `jarvis-tools`. Reporting them as one variant
/// would send a reader to the wrong crate.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GoogleToolError {
    /// The operation's identifier could not be qualified.
    #[error(transparent)]
    Id(#[from] ToolIdError),
    /// An operation's schema is missing or rejected.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// A required scope was rejected.
    #[error(transparent)]
    Scope(#[from] ScopeError),
    /// The definition was rejected by the contract's own validation.
    #[error(transparent)]
    Definition(#[from] ToolDefinitionError),
    /// An operation in the manifest has no schema in this module.
    ///
    /// **The refusal that keeps the two lists in step.** A permissive schema invented from the identifier
    /// would advertise a tool whose arguments are unconstrained, and the defect would surface as a model
    /// calling a tool with arguments nobody validated rather than as a build failure.
    #[error(
        "the manifest declares the operation `{operation}` but this module has no schema for it; add the \
         schemas and a title, or remove the operation from the manifest"
    )]
    NoSchema {
        /// The operation with no schema.
        operation: String,
    },
}

/// Returns the tool definitions for every operation the manifest declares.
///
/// # Errors
///
/// Returns [`GoogleToolError`] if an operation has no schema in this module, declares a risk above the
/// platform ceiling, or produces a contract violation. Every one of these is a defect in this repository
/// rather than a runtime condition, which is why the test module asserts the whole list is produced.
pub fn definitions(manifest: &ConnectorManifest) -> Result<Vec<ToolDefinition>, GoogleToolError> {
    let mut definitions = Vec::with_capacity(manifest.operations().len());
    for operation in manifest.operations() {
        definitions.push(definition(operation)?);
    }
    Ok(definitions)
}

/// Builds one tool definition from one manifest operation.
///
/// # Errors
///
/// Returns [`GoogleToolError`] as [`definitions`] does.
///
/// # Why there is no risk-ceiling check here
///
/// An earlier version had one, and it was **unreachable**: `ValidatedOperation` has private fields and is
/// only built by [`ConnectorManifest`](crate::manifest::ConnectorManifest)'s constructor, which already
/// refuses a risk above the platform's ceiling. So `operation.risk()` cannot exceed it, and a check here
/// would be a refusal that can never fire — the same defect `P5-001` and `P5-003` each recorded. Removing it
/// was the fix rather than keeping it as belt-and-braces, because an unreachable refusal reads as protection
/// while enforcing nothing.
pub fn definition(operation: &ValidatedOperation) -> Result<ToolDefinition, GoogleToolError> {
    let id = operation.id();
    // Four lookups, and each `None` is the same defect: the manifest declares an operation this module has
    // no contract for. Reported together rather than as four arms, because the fix is one edit either way.
    let (Some(input), Some(output)) = (input_schema(id), output_schema(id)) else {
        return Err(GoogleToolError::NoSchema {
            operation: id.to_owned(),
        });
    };
    let Some(title) = title(id) else {
        return Err(GoogleToolError::NoSchema {
            operation: id.to_owned(),
        });
    };
    // `Risk::from_level` cannot fail here for the reason above, and the error is reported rather than
    // unwrapped because `expect` is denied in this crate. It is the one branch in this file that no input can
    // reach.
    let risk = Risk::from_level(operation.risk()).map_err(|_| GoogleToolError::NoSchema {
        operation: id.to_owned(),
    })?;
    let effects = EffectSet::new(operation.effects().iter()).ok_or(GoogleToolError::NoSchema {
        // An empty effect set is refused by `EffectSet::new` and by the manifest's own constructor, so this
        // is unreachable through either.
        operation: id.to_owned(),
    })?;
    let idempotency = tool_idempotency(operation.idempotency());

    Ok(ToolDefinition::new(ToolDefinitionParts {
        // Namespaced under the connector's identifier, which is what makes this a `Connector` source rather
        // than a `Native` one: `ToolSource::from_namespace` classifies by the **leading segment**, so
        // `google.gmail_messages_read` is a connector tool while a bare `gmail_messages_read` would be
        // refused as unqualified.
        id: ToolId::new(format!("{}.{}", crate::google::CONNECTOR_ID, id))?,
        version: TOOL_VERSION.to_owned(),
        title: title.to_owned(),
        description: operation.description().to_owned(),
        input_schema: schema(input)?,
        output_schema: schema(output)?,
        effects,
        risk: risk.level(),
        required_scopes: caller_scopes(operation)?,
        // Derived from the **validated** risk rather than taken from the manifest, because the manifest has no
        // approval field. Risk 0 is `Auto` by the guidance table, and the manifest refuses an operation whose
        // risk is below its effects' floor — so deriving the policy cannot under-approve, and a future write
        // operation cannot be added with `Auto` by accident: it would have to raise its risk first.
        approval: ApprovalPolicy::for_risk(risk),
        timeout_seconds: TOOL_TIMEOUT_SECONDS,
        retry: retry_declaration(operation.idempotency()),
        idempotency,
        source: ToolSource::Connector,
        // Available, because nothing about a declaration is unavailable. Whether an account is connected is a
        // runtime fact and belongs to the health path, not to a static definition — `P3-002` records that the
        // restrictive of the two wins, so declaring `Available` here and `Unavailable` at runtime is correct.
        availability: Availability::Available,
        // The input is what a caller *sends* — a message id, a query, a calendar id — and the output is the
        // user's mail or calendar. Different levels, and a single field would have to be the maximum, which
        // would over-restrict the input and hide what the tool consumes.
        sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Confidential),
    })?)
}

/// Parses a schema constant under the required dialect.
///
/// The dialect check happens before parsing because `ToolSchema::from_value` would accept a document with no
/// `$schema` keyword as an error about a *different* problem; naming the dialect here points a reader at the
/// missing keyword. `jarvis-tools`' own `files.rs` does the same for its constants.
fn schema(text: &str) -> Result<ToolSchema, SchemaError> {
    if !text.contains(TOOL_SCHEMA_DIALECT) {
        return Err(SchemaError::WrongDialect { found: None });
    }
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| SchemaError::NotJson)?;
    ToolSchema::from_value(value)
}

/// The input schema of `gmail_messages_list`.
const GMAIL_LIST_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "query": {
      "type": "string",
      "description": "Gmail search syntax. Omit to list the newest messages.",
      "maxLength": 512
    },
    "max_results": {
      "type": "integer",
      "minimum": 1,
      "maximum": 500,
      "description": "Gmail caps this at 500; a larger value returns fewer without failing."
    },
    "page_token": { "type": "string", "maxLength": 4096 }
  },
  "additionalProperties": false
}"#;

/// The output schema of `gmail_messages_list`.
const GMAIL_LIST_OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "message_ids": { "type": "array", "items": { "type": "string" } },
    "next_page_token": { "type": ["string", "null"] }
  },
  "required": ["message_ids"],
  "additionalProperties": false
}"#;

/// The input schema of `gmail_history_list`.
const GMAIL_HISTORY_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "start_history_id": {
      "type": "string",
      "minLength": 1,
      "maxLength": 256,
      "description": "The mailbox position to list changes since. A value outside the retained range produces HTTP 404, which requires a full resync."
    },
    "max_results": { "type": "integer", "minimum": 1, "maximum": 500 },
    "page_token": { "type": "string", "maxLength": 4096 }
  },
  "required": ["start_history_id"],
  "additionalProperties": false
}"#;

/// The output schema of `gmail_history_list`.
const GMAIL_HISTORY_OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "message_ids": { "type": "array", "items": { "type": "string" } },
    "next_page_token": { "type": ["string", "null"] },
    "history_id": {
      "type": ["string", "null"],
      "description": "The mailbox's new position. This is the next sync cursor, and it is NOT the same field as next_page_token."
    }
  },
  "required": ["message_ids"],
  "additionalProperties": false
}"#;

/// The input schema of `gmail_profile_read`.
///
/// # Why this schema has no properties at all
///
/// Every other input here declares arguments, so an empty one reads as an omission. It is not: the operation
/// asks about **the credential that is calling**, and `users.getProfile`'s path parameter is documented as
/// *"The user's email address. The special value `me` can be used to indicate the authenticated user."* The
/// connector always sends `me`, so a `user_id` argument would be a field a caller could fill with another
/// mailbox — a request the caller's own token does not authorise, and one whose refusal would arrive as a
/// `403` rather than as a schema error. Leaving the properties out makes that call unrepresentable, which is
/// the same move the request builder makes by taking no arguments.
///
/// `additionalProperties: false` is what keeps the emptiness meaningful rather than merely undocumented: a
/// model that invented a field is refused rather than having it silently dropped, which is the failure
/// `ADR-0093` names for an argument the provider would ignore.
const GMAIL_PROFILE_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {},
  "additionalProperties": false
}"#;

/// The output schema of `gmail_profile_read`.
///
/// # Both fields are the provider's own statement about the account
///
/// The `users.getProfile` reference gives the response as `{ "emailAddress": string, "messagesTotal": integer,
/// "threadsTotal": integer, "historyId": string }`. This connector declares the two it renders: the address,
/// which is the identity `tools-and-connectors.md` requires to be "verified from the provider, not
/// user-entered labels", and the position, which a caller can use to seed a first sync.
///
/// **`messagesTotal` and `threadsTotal` are deliberately absent, not overlooked.** They are mailbox *counts* —
/// metadata about the account's size — and nothing in this connector consumes them. Declaring a field no
/// renderer produces is the defect `ADR-0083` records: a schema promising a value the tool never returns reads
/// as unreliable output. They can be added when something reads them.
///
/// **`email_address` is required and `history_id` is not**, matching `parse_profile`: a profile with no address
/// establishes nothing and is refused, while the reference documents `historyId` as a separate field a
/// caller may not need. The address is a person's mailbox, so it is rendered here under `Confidential` output
/// classification like the mail this connector reads — and it is the same class of value
/// `PubsubNotification` and `VerifiedAccount` both redact in `Debug` (`ADR-0091`).
const GMAIL_PROFILE_OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "email_address": {
      "type": "string",
      "description": "The connected mailbox's address, as the provider stated it. This is the account's verified identity, not a label a caller chose."
    },
    "history_id": {
      "type": ["string", "null"],
      "description": "The mailbox's position at the time of this read. Omitted when the provider did not return it. This is a mailbox POSITION and not the identity; it is also not the position a sync ends at."
    }
  },
  "required": ["email_address"],
  "additionalProperties": false
}"#;

/// The input schema of `gmail_messages_read`.
const GMAIL_READ_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "message_id": { "type": "string", "minLength": 1, "maxLength": 256 },
    "format": {
      "type": "string",
      "enum": ["minimal", "metadata", "full"],
      "description": "`raw` is deliberately absent: it returns the unparsed MIME message."
    }
  },
  "required": ["message_id"],
  "additionalProperties": false
}"#;

/// The output schema of `gmail_messages_read`.
///
/// # Why only `message_id` is required, and why there is no `snippet`
///
/// The provider returns **different top-level fields for different formats**: the Format page says `minimal`
/// returns "only email message ID and labels" and `metadata` returns "only email message ID, labels, and email
/// headers", while `full` returns the whole parsed resource. So `thread_id` and `label_ids` may legitimately be
/// absent and are declared optional — a `required` field a `minimal` read cannot fill would make an honest
/// response fail this schema.
///
/// `snippet` is **absent from the resource fields this tool reads** because no offered format returns it:
/// `minimal` and `metadata` do not, and the connector's default `full` would, but declaring a field two of the
/// three accepted formats never produce promises a value the tool cannot reliably deliver. It is omitted rather
/// than made optional, because "sometimes absent" for something genuinely unreachable reads as unreliable
/// output (see `request::GmailMessage`, `ADR-0083`).
const GMAIL_READ_OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "message_id": { "type": "string" },
    "thread_id": {
      "type": ["string", "null"],
      "description": "The thread the message belongs to. Omitted when the provider did not return it, which `minimal` and `metadata` may not."
    },
    "label_ids": {
      "type": ["array", "null"],
      "items": { "type": "string" },
      "description": "Labels applied to the message. Omitted when the provider did not return them; an empty array means the message carries no labels."
    }
  },
  "required": ["message_id"],
  "additionalProperties": false
}"#;

/// The input schema of `calendar_events_read`.
///
/// # Why the `time_min`/`time_max` descriptions carry the sync-token restriction
///
/// The `events.list` reference lists `timeMin` and `timeMax` among the parameters that "cannot be specified
/// together with nextSyncToken", and the connector refuses the pairing in `request::calendar_events_list`
/// (`ADR-0084`). The **schema is what a model reads**, so the restriction has to be visible here and not only
/// as a refusal at call time: a model that saw both fields advertised with no note would have no way to know
/// the combination it chose could never work. The `allOf`/`not` constraint below enforces the restriction for
/// a validator that runs the document; the prose is what a model reading the properties sees, and both are
/// present because they serve different readers.
const CALENDAR_READ_INPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "calendar_id": { "type": "string", "minLength": 1, "maxLength": 256 },
    "time_min": {
      "type": "string",
      "maxLength": 64,
      "description": "RFC 3339 lower bound. Cannot be combined with `sync_token`: the provider refuses a time range together with a sync token, because an incremental sync must repeat the initial request's filters."
    },
    "time_max": {
      "type": "string",
      "maxLength": 64,
      "description": "RFC 3339 upper bound. Cannot be combined with `sync_token`, for the same reason as `time_min`."
    },
    "max_results": { "type": "integer", "minimum": 1, "maximum": 2500 },
    "page_token": {
      "type": "string",
      "maxLength": 4096,
      "description": "Continues a paginated read. The provider returns `next_page_token` when a page is incomplete, and the same query must be repeated with it — including the same `sync_token`, which is how a large incremental sync is walked."
    },
    "sync_token": {
      "type": "string",
      "maxLength": 4096,
      "description": "Opaque. Invalidated tokens produce HTTP 410, which requires a full resync. Cannot be combined with `time_min` or `time_max`."
    }
  },
  "required": ["calendar_id"],
  "allOf": [
    {
      "not": {
        "required": ["sync_token", "time_min"]
      }
    },
    {
      "not": {
        "required": ["sync_token", "time_max"]
      }
    }
  ],
  "additionalProperties": false
}"#;

/// The output schema of `calendar_events_read`.
const CALENDAR_READ_OUTPUT: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "event_ids": { "type": "array", "items": { "type": "string" } },
    "next_sync_token": { "type": ["string", "null"] },
    "next_page_token": { "type": ["string", "null"] }
  },
  "required": ["event_ids"],
  "additionalProperties": false
}"#;

#[cfg(test)]
#[path = "definitions_tests.rs"]
mod tests;
