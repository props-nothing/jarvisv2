//! Durable storage for tool calls: the bounded invocation, its outcome, and the idempotency ledger.
//!
//! `docs/architecture/tools-and-connectors.md` states the two rules this module implements:
//!
//! - the execution pipeline ends at "Persist result and decision receipt", so a call is a durable
//!   record rather than a log line;
//! - "Do not turn a success-sounding string into proof. Preserve provider IDs and receipts separately
//!   from user-facing text", so the provider's locator and the tool's output are separate columns with
//!   very different bounds.
//!
//! # The idempotency ledger is the unique index, not a second table
//!
//! [`admit_tool_call`] generates the key once and inserts it. A re-driven pipeline that reaches
//! [`admit_tool_call`] again with the same key gets
//! [`DatabaseError::ToolCallDuplicate`] carrying the **existing** identifier, so the caller adopts the
//! existing call instead of making a second one. That is what "idempotency/duplicate check" means in
//! the pipeline diagram, and it is why the key is generated at admission rather than derived from the
//! intent: a deliberate second call with identical arguments gets a new key and is a second call.
//!
//! # An outcome is written once, and a terminal one cannot be replaced
//!
//! [`record_tool_outcome`] is a single guarded `UPDATE` requiring `version` to match. A losing writer
//! is reported as a conflict. **A terminal existing outcome is refused outright**, because the case
//! that matters is `Unknown` being quietly replaced by `Failed` after a re-drive: `Unknown` means the
//! effect may have happened, so overwriting it with "nothing happened" is how one sent message becomes
//! two.
//!
//! # Expiry and deadline comparisons happen in Rust
//!
//! `jarvis_core::UtcTimestamp`'s stored form is not lexicographically sortable (ADR-0018), so no
//! predicate here compares a timestamp string. Ordering is by `created_at` with the identifier as a
//! tie-break, which is stable without depending on the text form's collation.

use jarvis_core::{CorrelationId, ToolOutcome, ToolOutcomeRecord, UtcTimestamp};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// One stored tool call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredToolCall {
    id: String,
    workspace_id: String,
    run_id: String,
    step_id: Option<String>,
    tool: String,
    tool_version: String,
    intent_hash: String,
    idempotency_key: String,
    receipt: String,
    policy_version: String,
    approval_id: Option<String>,
    record: ToolOutcomeRecord,
    output: Option<String>,
    output_truncated: bool,
    correlation_id: CorrelationId,
    created_at: UtcTimestamp,
    updated_at: UtcTimestamp,
    reported_at: Option<UtcTimestamp>,
    version: i64,
}

impl StoredToolCall {
    /// Returns the call identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the owning workspace.
    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Returns the run that made the call.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Returns the step that made the call, when one is recorded.
    #[must_use]
    pub fn step_id(&self) -> Option<&str> {
        self.step_id.as_deref()
    }

    /// Returns the tool.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Returns the tool version the call was made against.
    #[must_use]
    pub fn tool_version(&self) -> &str {
        &self.tool_version
    }

    /// Returns the canonical intent digest.
    #[must_use]
    pub fn intent_hash(&self) -> &str {
        &self.intent_hash
    }

    /// Returns the idempotency key, which is also the ledger entry.
    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    /// Returns the authorization receipt document.
    #[must_use]
    pub fn receipt(&self) -> &str {
        &self.receipt
    }

    /// Returns the policy version that authorized the call.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    /// Returns the approval that authorized the call, when one was required.
    #[must_use]
    pub fn approval_id(&self) -> Option<&str> {
        self.approval_id.as_deref()
    }

    /// Returns the outcome record, with its evidence or reason.
    #[must_use]
    pub const fn record(&self) -> &ToolOutcomeRecord {
        &self.record
    }

    /// Returns the outcome state.
    #[must_use]
    pub const fn outcome(&self) -> ToolOutcome {
        self.record.outcome()
    }

    /// Returns the bounded tool output.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Returns whether the output was cut.
    #[must_use]
    pub const fn output_truncated(&self) -> bool {
        self.output_truncated
    }

    /// Returns the correlation identity.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns when the call was admitted.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }

    /// Returns when the row was last written.
    #[must_use]
    pub const fn updated_at(&self) -> UtcTimestamp {
        self.updated_at
    }

    /// Returns when the adapter reported, when it has.
    #[must_use]
    pub const fn reported_at(&self) -> Option<UtcTimestamp> {
        self.reported_at
    }

    /// Returns the optimistic-concurrency version.
    #[must_use]
    pub const fn version(&self) -> i64 {
        self.version
    }

    /// Returns whether a re-drive must not repeat this call.
    ///
    /// The predicate a recovery path needs: a call whose outcome **may have had an effect** is not
    /// repeatable, and neither is one still in flight. Everything else can be re-attempted. The answer
    /// is computed from the outcome rather than stored, so it cannot go stale.
    #[must_use]
    pub const fn must_not_repeat(&self) -> bool {
        self.record.outcome().may_have_had_an_effect() || !self.record.outcome().is_terminal()
    }
}

/// Where a call came from: the workspace, the run that made it, and the step within that run.
///
/// `_id` is what the schema calls these columns — each names a different foreign key to a different
/// table — so the shared suffix is the database's naming rather than accidental repetition. Renaming
/// them to satisfy the lint would make the field names disagree with the columns they bind to.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallOrigin {
    workspace_id: String,
    run_id: String,
    step_id: Option<String>,
}

impl CallOrigin {
    /// Binds a call to the run that made it, optionally to a step within that run.
    #[must_use]
    pub fn new(
        workspace_id: impl Into<String>,
        run_id: impl Into<String>,
        step_id: Option<String>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            run_id: run_id.into(),
            step_id,
        }
    }
}

/// What a call calls: the tool and the version it was resolved against.
///
/// The version is stored rather than derived, so a receipt can be revalidated against the definition
/// that actually decided, even after the registry moves on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallTarget {
    tool: String,
    tool_version: String,
}

impl CallTarget {
    /// Names the tool and version the call was authorized against.
    #[must_use]
    pub fn new(tool: impl Into<String>, tool_version: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            tool_version: tool_version.into(),
        }
    }
}

/// What a call is bound to: the authorization check's evidence, all five parts together.
///
/// These are stored as one value because an audit that has some of them cannot be revalidated. The
/// intent digest is what an approval binds to, the key is the ledger entry, the receipt is what
/// permitted the call, the policy version is what decided, and the approval identifier is what the
/// decision cites. Reading one without the others would let a reader conclude "authorized" from a
/// receipt that the current policy no longer issues.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallBinding {
    intent_hash: String,
    idempotency_key: String,
    receipt: String,
    policy_version: String,
    approval_id: Option<String>,
}

impl CallBinding {
    /// Binds a call to its authorization evidence.
    ///
    /// `idempotency_key` is generated by the caller at admission, not derived from the intent: a
    /// deliberate second call with identical arguments is a second call, and a derived key would
    /// silently collapse the two into one.
    #[must_use]
    pub fn new(
        intent_hash: impl Into<String>,
        idempotency_key: impl Into<String>,
        receipt: impl Into<String>,
        policy_version: impl Into<String>,
        approval_id: Option<String>,
    ) -> Self {
        Self {
            intent_hash: intent_hash.into(),
            idempotency_key: idempotency_key.into(),
            receipt: receipt.into(),
            policy_version: policy_version.into(),
            approval_id,
        }
    }
}

/// One call to admit.
///
/// Grouped into three concepts — origin, target, binding — rather than thirteen loose strings, because
/// the five binding fields are all short opaque text and a transposed pair would still validate. The
/// grouping does not make a swap *impossible*, but it puts each value next to the one other value it
/// could sensibly be confused with, instead of next to twelve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewToolCall {
    id: String,
    origin: CallOrigin,
    target: CallTarget,
    binding: CallBinding,
    correlation_id: CorrelationId,
    created_at: UtcTimestamp,
}

impl NewToolCall {
    /// Validates the fields required to admit a call.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError::InvalidToolCallRequest`] when an identifier is not UUID-sized, the
    /// intent hash is not a 64-character lowercase digest, the idempotency key is not 32 lowercase
    /// hex characters, or the receipt or policy version is unusable.
    ///
    /// The digest and key shapes are re-checked here rather than trusted, because both are compared as
    /// text elsewhere: a malformed digest would be *stored* happily and then never match a recomputed
    /// one, which fails silently at the point where an approval is supposed to be revalidated.
    pub fn new(
        id: impl Into<String>,
        origin: CallOrigin,
        target: CallTarget,
        binding: CallBinding,
        correlation_id: CorrelationId,
        created_at: UtcTimestamp,
    ) -> Result<Self, DatabaseError> {
        let fields = AdmittedFields {
            id: id.into(),
            workspace_id: origin.workspace_id,
            run_id: origin.run_id,
            step_id: origin.step_id,
            tool: target.tool,
            tool_version: target.tool_version,
            intent_hash: binding.intent_hash,
            idempotency_key: binding.idempotency_key,
            receipt: binding.receipt,
            policy_version: binding.policy_version,
            approval_id: binding.approval_id,
        };
        fields.validate()?;
        Ok(Self {
            id: fields.id,
            origin: CallOrigin {
                workspace_id: fields.workspace_id,
                run_id: fields.run_id,
                step_id: fields.step_id,
            },
            target: CallTarget {
                tool: fields.tool,
                tool_version: fields.tool_version,
            },
            binding: CallBinding {
                intent_hash: fields.intent_hash,
                idempotency_key: fields.idempotency_key,
                receipt: fields.receipt,
                policy_version: fields.policy_version,
                approval_id: fields.approval_id,
            },
            correlation_id,
            created_at,
        })
    }

    /// Returns the call identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the idempotency key.
    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.binding.idempotency_key
    }
}

/// The text fields one admission validates, grouped so the checks are in one place.
struct AdmittedFields {
    id: String,
    workspace_id: String,
    run_id: String,
    step_id: Option<String>,
    tool: String,
    tool_version: String,
    intent_hash: String,
    idempotency_key: String,
    receipt: String,
    policy_version: String,
    approval_id: Option<String>,
}

impl AdmittedFields {
    /// Validates every bounded field, reporting the first problem by name.
    fn validate(&self) -> Result<(), DatabaseError> {
        let invalid = |field: &'static str| DatabaseError::InvalidToolCallRequest { field };
        for (field, value) in [
            ("id", &self.id),
            ("workspace_id", &self.workspace_id),
            ("run_id", &self.run_id),
        ] {
            if value.len() != 36 {
                return Err(invalid(field));
            }
        }
        if let Some(step_id) = &self.step_id
            && step_id.len() != 36
        {
            return Err(invalid("step_id"));
        }
        if let Some(approval_id) = &self.approval_id
            && approval_id.len() != 36
        {
            return Err(invalid("approval_id"));
        }
        if self.tool.len() < 3 || self.tool.len() > 128 {
            return Err(invalid("tool"));
        }
        if self.tool_version.is_empty() || self.tool_version.len() > 32 {
            return Err(invalid("tool_version"));
        }
        if !is_lowercase_hex(&self.intent_hash, 64) {
            return Err(invalid("intent_hash"));
        }
        if !is_lowercase_hex(&self.idempotency_key, 32) {
            return Err(invalid("idempotency_key"));
        }
        if self.receipt.len() < 2 || self.receipt.len() > 4096 {
            return Err(invalid("receipt"));
        }
        if self.policy_version.is_empty() || self.policy_version.len() > 64 {
            return Err(invalid("policy_version"));
        }
        Ok(())
    }
}

/// Returns whether a value is exactly `length` lowercase hexadecimal characters.
fn is_lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Admits a tool call, or reports the existing call for the same idempotency key.
///
/// # Errors
///
/// - [`DatabaseError::InvalidToolCallRequest`] when a field fails bounded validation.
/// - [`DatabaseError::ToolCallDuplicate`] when this run already has a call under this key. **The
///   caller should adopt the returned identifier**, not retry: this is the duplicate detection the
///   pipeline diagram places before execution.
/// - [`DatabaseError::Sqlite`] when a referenced workspace, run, step, or approval is missing.
pub async fn admit_tool_call(
    database: &SqliteDatabase,
    new: &NewToolCall,
) -> Result<String, DatabaseError> {
    let result = sqlx::query(
        "INSERT INTO tool_calls (\
            id, workspace_id, run_id, step_id, tool, tool_version, intent_hash, idempotency_key, \
            receipt, policy_version, approval_id, outcome, correlation_id, created_at, updated_at, \
            version\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'requested', ?12, ?13, ?13, 1) \
         ON CONFLICT (run_id, idempotency_key) DO NOTHING",
    )
    .bind(&new.id)
    .bind(&new.origin.workspace_id)
    .bind(&new.origin.run_id)
    .bind(new.origin.step_id.as_deref())
    .bind(&new.target.tool)
    .bind(&new.target.tool_version)
    .bind(&new.binding.intent_hash)
    .bind(&new.binding.idempotency_key)
    .bind(&new.binding.receipt)
    .bind(&new.binding.policy_version)
    .bind(new.binding.approval_id.as_deref())
    .bind(new.correlation_id.to_string())
    .bind(new.created_at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "admit a tool call",
        source,
    })?;

    if result.rows_affected() == 0 {
        // The conflict clause suppressed the insert, so a call already exists under this key. The
        // existing identifier is returned so the caller adopts it rather than guessing.
        let existing = find_call_by_idempotency_key(
            database,
            &new.origin.run_id,
            &new.binding.idempotency_key,
        )
        .await?
        .ok_or(DatabaseError::Sqlite {
            operation: "resolve a tool call duplicate",
            source: sqlx::Error::RowNotFound,
        })?;
        return Err(DatabaseError::ToolCallDuplicate {
            existing_call_id: existing.id,
        });
    }

    Ok(new.id.clone())
}

/// Finds a run's call by its idempotency key.
async fn find_call_by_idempotency_key(
    database: &SqliteDatabase,
    run_id: &str,
    key: &str,
) -> Result<Option<StoredToolCall>, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, run_id, step_id, tool, tool_version, intent_hash, \
                idempotency_key, receipt, policy_version, approval_id, outcome, evidence, output, \
                output_truncated, reason, correlation_id, created_at, updated_at, reported_at, \
                version \
         FROM tool_calls WHERE run_id = ?1 AND idempotency_key = ?2",
    )
    .bind(run_id)
    .bind(key)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a tool call by idempotency key",
        source,
    })?;
    row.as_ref().map(decode_tool_call).transpose()
}

/// Reads one tool call by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::ToolCallNotFound`] when no call has that identifier, or
/// [`DatabaseError::StoredToolCallInvalid`] when a stored row contradicts the domain's rules.
pub async fn find_tool_call(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredToolCall, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, run_id, step_id, tool, tool_version, intent_hash, \
                idempotency_key, receipt, policy_version, approval_id, outcome, evidence, output, \
                output_truncated, reason, correlation_id, created_at, updated_at, reported_at, \
                version \
         FROM tool_calls WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a tool call",
        source,
    })?
    .ok_or(DatabaseError::ToolCallNotFound)?;
    decode_tool_call(&row)
}

/// Reads a run's tool calls, oldest first.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredToolCallInvalid`] when any stored row cannot be decoded, so one
/// corrupt row is reported rather than silently skipped.
pub async fn read_run_tool_calls(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Vec<StoredToolCall>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, workspace_id, run_id, step_id, tool, tool_version, intent_hash, \
                idempotency_key, receipt, policy_version, approval_id, outcome, evidence, output, \
                output_truncated, reason, correlation_id, created_at, updated_at, reported_at, \
                version \
         FROM tool_calls WHERE run_id = ?1 ORDER BY created_at ASC, id ASC",
    )
    .bind(run_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a run's tool calls",
        source,
    })?;
    rows.iter().map(decode_tool_call).collect()
}

/// Reads the calls a re-drive must not repeat.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredToolCallInvalid`] when a stored row cannot be decoded.
///
/// The filter is applied in Rust rather than SQL because it is a rule about outcomes
/// (`may_have_had_an_effect` or not yet terminal), and encoding that rule in a predicate would give it
/// a second home where it could drift from the domain.
pub async fn read_unrepeatable_calls(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Vec<StoredToolCall>, DatabaseError> {
    Ok(read_run_tool_calls(database, run_id)
        .await?
        .into_iter()
        .filter(StoredToolCall::must_not_repeat)
        .collect())
}

/// Records an outcome against an admitted call.
///
/// # Errors
///
/// - [`DatabaseError::ToolCallNotFound`] when no call has that identifier.
/// - [`DatabaseError::ToolCallAlreadyResolved`] when the stored outcome is already terminal and this
///   outcome differs. **A recorded effect is not re-reportable**: replacing it is how an `Unknown`
///   becomes a `Failed` and one sent message becomes two.
/// - [`DatabaseError::ToolCallConflict`] when the guarded write matched nothing because another writer
///   advanced the version.
/// - [`DatabaseError::StoredToolCallInvalid`] when the stored row cannot be decoded.
///
/// A **terminal stored outcome with the same value** is accepted as a no-op, so a retried write of the
/// same fact is not an error while a changed fact is.
pub async fn record_tool_outcome(
    database: &SqliteDatabase,
    id: &str,
    record: &ToolOutcomeRecord,
    output: Option<(&str, bool)>,
    at: UtcTimestamp,
) -> Result<StoredToolCall, DatabaseError> {
    let existing = find_tool_call(database, id).await?;

    // The domain owns the transition table; this re-uses it rather than restating the edges.
    if existing.outcome().is_terminal() {
        if existing.outcome() == record.outcome() {
            return Ok(existing);
        }
        return Err(DatabaseError::ToolCallAlreadyResolved {
            existing: existing.outcome().as_str().to_owned(),
        });
    }
    if !existing.outcome().can_advance_to(record.outcome()) {
        return Err(DatabaseError::StoredToolCallInvalid { field: "outcome" });
    }

    // A state the adapter reported stamps `reported_at`; a state the pipeline reached before handing
    // anything to an adapter leaves it null, matching the migration's CHECK. The predicate is the
    // domain's, so the two cannot drift — and `Submitted` is a report, which a predicate phrased as
    // "terminal outcomes were reported" would have got wrong.
    let reported_at = record.outcome().is_reported().then(|| at.to_string());
    let (output_text, output_truncated) = match output {
        Some((text, truncated)) => (Some(text.to_owned()), truncated),
        None => (None, false),
    };

    let result = sqlx::query(
        "UPDATE tool_calls SET \
            outcome = ?3, evidence = ?4, reason = ?5, output = ?6, output_truncated = ?7, \
            reported_at = ?8, updated_at = ?9, version = version + 1 \
         WHERE id = ?1 AND version = ?2",
    )
    .bind(id)
    .bind(existing.version)
    .bind(record.outcome().as_str())
    .bind(record.evidence())
    .bind(record.reason())
    .bind(output_text)
    .bind(i64::from(output_truncated))
    .bind(reported_at)
    .bind(at.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a tool outcome",
        source,
    })?;

    if result.rows_affected() == 0 {
        return Err(DatabaseError::ToolCallConflict);
    }

    find_tool_call(database, id).await
}

/// Moves an admitted call to `authorized` or `submitted`, with no output attached.
///
/// A convenience for the states the pipeline reaches before anything is known about an effect, and the
/// legal-step helper for callers that only want to advance. Transitions are still checked, so
/// `requested -> submitted` is refused: a call that never passed `authorized` has no receipt, and an
/// adapter must never be handed one.
///
/// # Errors
///
/// Returns the same failures as [`record_tool_outcome`] for the same reasons.
pub async fn advance_tool_call(
    database: &SqliteDatabase,
    id: &str,
    outcome: ToolOutcome,
    at: UtcTimestamp,
) -> Result<StoredToolCall, DatabaseError> {
    let record = ToolOutcomeRecord::new(outcome)
        .map_err(|_| DatabaseError::InvalidToolCallRequest { field: "outcome" })?;
    record_tool_outcome(database, id, &record, None, at).await
}

/// Decodes one stored row, re-checking the domain's invariants.
///
/// The migration's `CHECK`s already make a contradictory row unstorable through this repository, but a
/// row written by another build, restored from a backup, or hand-edited is not covered by that
/// argument — the same reasoning the run and approval repositories use for their own re-checks.
fn decode_tool_call(row: &sqlx::sqlite::SqliteRow) -> Result<StoredToolCall, DatabaseError> {
    let invalid = |field: &'static str| DatabaseError::StoredToolCallInvalid { field };
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field).map_err(|_| invalid(field))
    };
    let optional = |field: &'static str| -> Result<Option<String>, DatabaseError> {
        row.try_get::<Option<String>, _>(field)
            .map_err(|_| invalid(field))
    };

    let id = text("id")?;
    let workspace_id = text("workspace_id")?;
    let run_id = text("run_id")?;
    let step_id = optional("step_id")?;
    let tool = text("tool")?;
    let tool_version = text("tool_version")?;
    let intent_hash = text("intent_hash")?;
    let idempotency_key = text("idempotency_key")?;
    let receipt = text("receipt")?;
    let policy_version = text("policy_version")?;
    let approval_id = optional("approval_id")?;
    let outcome_text = text("outcome")?;
    let evidence = optional("evidence")?;
    let output = optional("output")?;
    let reason = optional("reason")?;
    let correlation_id = text("correlation_id")?;
    let created_at = text("created_at")?;
    let updated_at = text("updated_at")?;
    let reported_at = optional("reported_at")?;

    let truncated = row
        .try_get::<i64, _>("output_truncated")
        .map_err(|_| invalid("output_truncated"))?;
    let output_truncated = match truncated {
        0 => false,
        1 => true,
        _ => return Err(invalid("output_truncated")),
    };
    let version = row
        .try_get::<i64, _>("version")
        .map_err(|_| invalid("version"))?;
    if version < 1 {
        return Err(invalid("version"));
    }

    let outcome: ToolOutcome = outcome_text.parse().map_err(|_| invalid("outcome"))?;
    let record = ToolOutcomeRecord::from_stored(outcome, evidence.as_deref(), reason.as_deref())
        .map_err(|_| invalid("outcome"))?;

    // The pairing the migration enforces, re-checked: a truncation flag without output is meaningless.
    if output.is_none() && output_truncated {
        return Err(invalid("output_truncated"));
    }

    let parse = |value: &str, field: &'static str| -> Result<UtcTimestamp, DatabaseError> {
        value.parse().map_err(|_| invalid(field))
    };

    let correlation: CorrelationId = correlation_id
        .parse()
        .map_err(|_| invalid("correlation_id"))?;

    Ok(StoredToolCall {
        id,
        workspace_id,
        run_id,
        step_id,
        tool,
        tool_version,
        intent_hash,
        idempotency_key,
        receipt,
        policy_version,
        approval_id,
        record,
        output,
        output_truncated,
        correlation_id: correlation,
        created_at: parse(&created_at, "created_at")?,
        updated_at: parse(&updated_at, "updated_at")?,
        reported_at: reported_at
            .as_deref()
            .map(|value| parse(value, "reported_at"))
            .transpose()?,
        version,
    })
}

#[cfg(test)]
#[path = "tool_call_tests.rs"]
mod tests;
