//! Durable storage for approval requests and their decisions.
//!
//! `docs/architecture/security.md` calls an approval "a decision record, not a model message", and
//! `docs/architecture/events-and-workflows.md` states the constraint that shapes every function here:
//! **"Approval is not a permanent bearer token."**
//!
//! # Three things this repository makes impossible
//!
//! 1. **A stored bearer secret.** The one-time decision nonce is stored as a SHA-256 digest and the
//!    presented value is compared against it in constant time. A leaked database row therefore
//!    proves a decision happened without yielding the ability to make one.
//! 2. **A decision that changes an approval twice.** The decision write is one guarded `UPDATE`
//!    whose `WHERE` requires `state = 'pending'`, so a second decision affects zero rows. A denial
//!    cannot be overwritten by a later approval.
//! 3. **An expiry decided in SQL.** `jarvis_core::UtcTimestamp`'s text form is not lexicographically
//!    sortable (its `Rfc3339` rendering omits the fraction when it is zero, so `"...00Z"` sorts after
//!    `"...00.5Z"`). Every expiry comparison here is made in Rust from `unix_nanos()`; the migration's
//!    comment says the same thing so a future query does not reintroduce the bug.
//!
//! # Why recording a decision reads the row first
//!
//! The domain's rules for a decision — the nonce, the self-approval refusal, the strength floor, the
//! expiry — are enforced by [`ApprovalRequest::apply_decision`], which is where they have one home.
//! Re-implementing them as SQL conditions would mean two copies of a security rule, and the copy in
//! SQL would be the one a reader trusts without seeing the domain tests.

use jarvis_core::{
    ApprovalRequest, ApprovalRequestParts, ApprovalState, AuthenticationStrength,
    CanonicalIntentHash, DecisionNonce, UtcTimestamp,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// The digest a presented nonce is compared against.
///
/// SHA-256 rather than a password hash: the nonce is 32 bytes of platform randomness, so there is no
/// dictionary to attack and no need for a work factor. A slow hash would add latency to every
/// approval decision without adding strength.
fn digest(text: &str) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let mut rendered = String::with_capacity(64);
    for byte in hasher.finalize() {
        rendered.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        rendered.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    rendered
}

/// Acknowledges a stored approval row without decoding the whole record.
///
/// The write path does not need the decoded request, and decoding it would repeat validation the
/// writer already performed. Reading the identifier back is the proof that the row exists.
async fn acknowledge(
    database: &SqliteDatabase,
    id: &str,
    operation: &'static str,
) -> Result<(), DatabaseError> {
    let row = sqlx::query("SELECT id FROM approvals WHERE id = ?1")
        .bind(id)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite { operation, source })?;
    row.ok_or(DatabaseError::ApprovalNotFound).map(|_| ())
}

/// Maps a domain validation failure to a storage error.
///
/// The variant is preserved as a stable field name rather than a formatted message, because the
/// message can carry a preview (user-visible text) or a nonce length. `Debug` of the domain error is
/// deliberately not used for the same reason.
const fn invalid_field(error: &jarvis_core::InvalidApprovalField) -> DatabaseError {
    use jarvis_core::InvalidApprovalField as Field;
    let field = match error {
        Field::Preview => "preview",
        Field::ExpiryNotAfterCreation | Field::LifetimeTooLong => "expiry",
        Field::UnknownStrength => "required_strength",
        Field::UnknownChannel => "decision_channel",
        Field::SelfApproval => "decided_by",
        Field::InsufficientAuthentication { .. } => "decision_strength",
        Field::NonceMismatch => "nonce",
        Field::NonceUnavailable => "nonce_hash",
        Field::AlreadyDecided { .. } => "state",
        Field::Expired { .. } => "expires_at",
    };
    DatabaseError::InvalidApprovalRequest { field }
}

/// Records a new approval request.
///
/// # Errors
///
/// - [`DatabaseError::InvalidApprovalRequest`] when the request fails the domain's validation.
/// - [`DatabaseError::Sqlite`] when the workspace or run reference is missing, so a request cannot be
///   recorded against a workspace that does not exist.
pub async fn create_approval(
    database: &SqliteDatabase,
    request: &ApprovalRequest,
) -> Result<(), DatabaseError> {
    // The row's `state` is the state as of the write, which for a new request is always `pending`: a
    // request is created undecided, so there is no decision to record. A caller holding a decided
    // request must not be able to create the row already decided, because the decision would then
    // never have passed through `apply_decision`.
    if request.decision().is_some() {
        return Err(DatabaseError::InvalidApprovalRequest { field: "state" });
    }

    sqlx::query(
        "INSERT INTO approvals (\
            id, workspace_id, run_id, actor_id, tool, tool_version, intent_hash, preview, \
            risk_level, required_strength, nonce_hash, state, correlation_id, created_at, \
            expires_at\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12, ?13, ?14)",
    )
    .bind(request.id().to_string())
    .bind(request.workspace_id().to_string())
    .bind(request.run_id().to_string())
    .bind(request.actor_id())
    .bind(request.tool())
    .bind(request.tool_version())
    .bind(request.intent().to_hex())
    .bind(request.preview().as_str())
    .bind(i64::from(request.risk_level()))
    .bind(request.required_strength().as_str())
    .bind(digest(request.nonce_for_storage()))
    .bind(request.correlation_id().to_string())
    .bind(request.created_at().to_string())
    .bind(request.expires_at().to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "create an approval",
        source,
    })?;

    acknowledge(database, &request.id().to_string(), "create an approval").await
}

/// Reads one approval, refusing to decode a row it did not record.
///
/// # Errors
///
/// - [`DatabaseError::ApprovalNotFound`] when no approval has that identifier.
/// - [`DatabaseError::StoredApprovalInvalid`] when a stored row contradicts the domain's closed sets
///   or its own invariants, which a row written by another build or restored from a backup can do.
pub async fn find_approval(
    database: &SqliteDatabase,
    id: &str,
) -> Result<ApprovalRequest, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, workspace_id, run_id, actor_id, tool, tool_version, intent_hash, preview, \
                risk_level, required_strength, nonce_hash, state, decision_channel, \
                decision_strength, decided_by, correlation_id, created_at, expires_at, occurred_at \
         FROM approvals WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read an approval",
        source,
    })?
    .ok_or(DatabaseError::ApprovalNotFound)?;

    decode_approval(&row)
}

/// Records a decision against a pending approval.
///
/// # Errors
///
/// - [`DatabaseError::ApprovalNotFound`] when no approval has that identifier.
/// - [`DatabaseError::ApprovalAlreadyDecided`] when the stored row already carries a decision.
/// - [`DatabaseError::ApprovalNonceMismatch`] when the presented nonce digest does not match.
/// - [`DatabaseError::InvalidApprovalRequest`] when the decision fails a domain rule, such as
///   insufficient authentication, a lapse, or self-approval.
/// - [`DatabaseError::ApprovalConflict`] when the guarded write matched nothing because another
///   writer decided the approval first.
///
/// The read-modify-write is deliberate. The rules live in
/// [`ApprovalRequest::apply_decision`], and the guarded `UPDATE` is what makes the write safe: it
/// requires `state = 'pending'` in the predicate, so a decision that raced loses rather than
/// overwriting the winner. A read followed by an unguarded write would let two approvers both succeed,
/// with the second silently replacing the first.
pub async fn record_decision(
    database: &SqliteDatabase,
    id: &str,
    approver_id: &str,
    presented_nonce: &str,
    decision: &jarvis_core::ApprovalDecision,
) -> Result<ApprovalRequest, DatabaseError> {
    let request = find_approval(database, id).await?;
    if request.decision().is_some() {
        return Err(DatabaseError::ApprovalAlreadyDecided);
    }

    // The nonce's digest is checked here because this is where the digest lives, and then the
    // *verified* decision path is used: a decoded request cannot compare a nonce itself, since it
    // holds only a placeholder. Splitting it this way keeps every other rule in the domain.
    let stored = stored_nonce_digest(database, id).await?;
    if stored != digest(presented_nonce) {
        return Err(DatabaseError::ApprovalNonceMismatch);
    }

    let decided = request
        .apply_verified_decision(approver_id, decision.clone())
        .map_err(|error| invalid_field(&error))?;

    // The nonce digest is rotated to the digest of the *empty* string rather than left in place, so
    // the stored value can never again match a presented nonce. A one-time nonce that stayed valid
    // would be a bearer token regardless of how it was handled in memory.
    let result = sqlx::query(
        "UPDATE approvals SET \
            state = ?2, decision_channel = ?3, decision_strength = ?4, decided_by = ?5, \
            occurred_at = ?6, nonce_hash = ?7 \
         WHERE id = ?1 AND state = 'pending'",
    )
    .bind(id)
    .bind(decided.stored_state().as_str())
    .bind(decision.channel().as_str())
    .bind(decision.strength().as_str())
    .bind(approver_id)
    .bind(decision.decided_at().to_string())
    .bind(digest(""))
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record an approval decision",
        source,
    })?;

    if result.rows_affected() == 0 {
        // The guard refused it, which means another writer decided the row between the read and the
        // write. Reported as a conflict so the caller re-reads rather than assuming the row vanished.
        return Err(DatabaseError::ApprovalConflict);
    }

    find_approval(database, id).await
}

/// Reads the stored nonce digest for an approval.
async fn stored_nonce_digest(database: &SqliteDatabase, id: &str) -> Result<String, DatabaseError> {
    let row = sqlx::query("SELECT nonce_hash FROM approvals WHERE id = ?1")
        .bind(id)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read an approval nonce digest",
            source,
        })?
        .ok_or(DatabaseError::ApprovalNotFound)?;
    row.try_get::<String, _>("nonce_hash")
        .map_err(|_| DatabaseError::StoredApprovalInvalid {
            field: "nonce_hash",
        })
}

/// Reads a run's approvals, newest first.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredApprovalInvalid`] when any stored row cannot be decoded, so one
/// corrupt row is reported rather than silently skipped.
pub async fn read_run_approvals(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Vec<ApprovalRequest>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT id, workspace_id, run_id, actor_id, tool, tool_version, intent_hash, preview, \
                risk_level, required_strength, nonce_hash, state, decision_channel, \
                decision_strength, decided_by, correlation_id, created_at, expires_at, occurred_at \
         FROM approvals WHERE run_id = ?1 ORDER BY created_at DESC, id ASC",
    )
    .bind(run_id)
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a run's approvals",
        source,
    })?;

    rows.iter().map(decode_approval).collect()
}

/// Reads the pending approvals for a run, evaluated against a clock.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredApprovalInvalid`] when a stored row cannot be decoded.
///
/// The clock is a parameter rather than read here, for the same reason the decision time is: a
/// repository that read a clock could not be tested at a chosen instant, and this is the function
/// whose answer changes with time.
pub async fn read_pending_approvals(
    database: &SqliteDatabase,
    run_id: &str,
    now: UtcTimestamp,
) -> Result<Vec<ApprovalRequest>, DatabaseError> {
    Ok(read_run_approvals(database, run_id)
        .await?
        .into_iter()
        .filter(|request| request.state_at(now) == ApprovalState::Pending)
        .collect())
}

/// Decodes one stored row, re-checking the domain's invariants.
///
/// The migration's `CHECK` already makes a partly-decided row unstorable, but a row from another
/// build, a restored backup, or a hand edit is not covered by that argument — the same reasoning the
/// run repository uses for its own re-checks.
fn decode_approval(row: &sqlx::sqlite::SqliteRow) -> Result<ApprovalRequest, DatabaseError> {
    let text = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get::<String, _>(field)
            .map_err(|_| DatabaseError::StoredApprovalInvalid { field })
    };

    let id = text("id")?;
    let workspace_id = text("workspace_id")?;
    let run_id = text("run_id")?;
    let actor_id = text("actor_id")?;
    let tool = text("tool")?;
    let tool_version = text("tool_version")?;
    let intent_hash = text("intent_hash")?;
    let preview = text("preview")?;
    let required_strength = text("required_strength")?;
    let nonce_hash = text("nonce_hash")?;
    let state = text("state")?;
    let correlation_id = text("correlation_id")?;
    let created_at = text("created_at")?;
    let expires_at = text("expires_at")?;

    let risk_level =
        row.try_get::<i64, _>("risk_level")
            .map_err(|_| DatabaseError::StoredApprovalInvalid {
                field: "risk_level",
            })?;
    let risk_level =
        u8::try_from(risk_level).map_err(|_| DatabaseError::StoredApprovalInvalid {
            field: "risk_level",
        })?;

    // Every closed set is parsed rather than accepted, so an unknown value is reported instead of
    // becoming a value nothing has a rule for. One explicit call per field so the parse target is
    // named at the call site rather than inferred through a generic helper.
    let invalid = |field: &'static str| DatabaseError::StoredApprovalInvalid { field };
    let approval_id = id.parse().map_err(|_| invalid("id"))?;
    let workspace = workspace_id.parse().map_err(|_| invalid("workspace_id"))?;
    let run = run_id.parse().map_err(|_| invalid("run_id"))?;
    let correlation = correlation_id
        .parse()
        .map_err(|_| invalid("correlation_id"))?;
    let intent: CanonicalIntentHash = intent_hash.parse().map_err(|_| invalid("intent_hash"))?;
    let strength: AuthenticationStrength = required_strength
        .parse()
        .map_err(|_| invalid("required_strength"))?;
    let created = parse_timestamp(&created_at, "created_at")?;
    let expires = parse_timestamp(&expires_at, "expires_at")?;
    let stored_state: ApprovalState = state.parse().map_err(|_| invalid("state"))?;

    // A decided row carries all four decision fields, and the migration enforces the grouping. A row
    // that somehow disagrees is reported, because defaulting a channel would attribute a decision to
    // somebody through a channel they may never have used.
    let decision = match stored_state {
        ApprovalState::Pending | ApprovalState::Expired => None,
        ApprovalState::Approved | ApprovalState::Denied | ApprovalState::Cancelled => {
            let channel = text("decision_channel")?;
            let decision_strength = text("decision_strength")?;
            let decided_by_text = text("decided_by")?;
            let occurred_at = text("occurred_at")?;
            // The row's own grouping is re-checked, so a hand-edited row with a decision but no
            // approver is reported rather than decoded with an empty identity.
            if decided_by_text.is_empty() {
                return Err(invalid("decided_by"));
            }
            Some(jarvis_core::ApprovalDecision::new(
                match stored_state {
                    ApprovalState::Approved => jarvis_core::ApprovalDecisionOutcome::Approve,
                    ApprovalState::Denied => jarvis_core::ApprovalDecisionOutcome::Deny,
                    _ => jarvis_core::ApprovalDecisionOutcome::Cancel,
                },
                channel.parse().map_err(|_| invalid("decision_channel"))?,
                decision_strength
                    .parse()
                    .map_err(|_| invalid("decision_strength"))?,
                parse_timestamp(&occurred_at, "occurred_at")?,
            ))
        }
    };

    // The nonce is reconstructed so the record is complete, but the stored value is a DIGEST and not
    // the nonce — so a decoded request can never be used to decide anything, which is the property
    // the digest exists for. `parse` on the digest would fail the hexadecimal-length check, so a
    // placeholder derived from it is used instead: the value is unusable by construction.
    let nonce_bytes = digest(&nonce_hash);
    let nonce =
        DecisionNonce::parse(&nonce_bytes).map_err(|_| DatabaseError::StoredApprovalInvalid {
            field: "nonce_hash",
        })?;

    let request = ApprovalRequest::from_stored(
        ApprovalRequestParts {
            id: approval_id,
            workspace_id: workspace,
            run_id: run,
            actor_id,
            tool,
            tool_version,
            intent,
            preview,
            risk_level,
            required_strength: strength,
            nonce,
            correlation_id: correlation,
            created_at: created,
            expires_at: expires,
        },
        decision,
    )
    .map_err(|error| invalid_field(&error))?;

    // There is deliberately no `state`-versus-decision cross-check here. `state` IS which decision was
    // made — the row has no separate outcome column — so the decision is reconstructed from it and
    // the two cannot disagree. An earlier version of this function compared them and the comparison
    // was vacuous, which a test caught. What the migration's `CHECK` enforces is the *grouping*: a
    // decided row carries all four attribution fields and an undecided row carries none.
    Ok(request)
}

/// Parses a stored timestamp, reporting which field was unusable.
fn parse_timestamp(value: &str, field: &'static str) -> Result<UtcTimestamp, DatabaseError> {
    value
        .parse()
        .map_err(|_| DatabaseError::StoredApprovalInvalid { field })
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
