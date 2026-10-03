//! Durable storage for session summaries (`P4-015`).
//!
//! A summary is recorded as a `memories` row and, separately, a `session_summaries` row naming the turns it
//! compressed. `SessionSummary::into_record` produces the memory; this adapter writes both halves **in one
//! transaction** and answers the one question the writer cannot answer alone: has this session already been
//! summarized here?
//!
//! # The overlap check is read-then-write, and that is why it must be one transaction
//!
//! The write refuses to record a summary of turns another summary already covers. That check is a `SELECT`
//! followed by two inserts, and a deferred transaction that reads and then writes fails with
//! `SQLITE_BUSY_SNAPSHOT` if another connection commits in between — the failure `purge_memory` records in its
//! own note, found the same way. So the check and both writes run on **one** connection, through this one
//! function, and a caller cannot split them.
//!
//! # What this module deliberately does not do
//!
//! It does not compose the summary text. Compression is a model's job and the *quality* of the text is not
//! something a schema can judge; what the schema and this adapter enforce is that whatever text arrives is
//! **attributed**: `Conversation` type, `Document` source kind (so `Derived` trust), a locator naming the
//! session and the exact span, and the span recorded against the transcript it claims. That is the checkable
//! half of "is never presented as user-authored fact".
//!
//! It also does not delete a summary a newer one subsumes. Supersession is a status transition on the memory
//! (`MemoryRecord::supersede`), and the overlap check refuses to *create* the overlap — the cheaper moment to
//! notice it, and the one where the reason is still visible.

use jarvis_core::{
    MemoryRecord, MemorySearchKey, SessionId, SessionSummary, SummaryLoss, SummarySpan,
    UtcTimestamp, spans_overlap,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};
use crate::memory_repository::{insert_memory_on, invalid_memory_field};

/// One stored summary, with the span and loss that make it a summary.
///
/// # Why the span and loss are returned and not only the memory identifier
///
/// They are what a caller needs to decide anything about a summary, and they are exactly what no `memories`
/// column holds. A consumer that had to reconstruct the span from the source locator would be parsing a
/// string this module formats — a contract kept in two places and broken by editing either.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSummary {
    memory_id: String,
    span: SummarySpan,
    loss: SummaryLoss,
    created_at: UtcTimestamp,
}

impl StoredSummary {
    /// Returns the identifier of the `memories` row holding the summary text.
    #[must_use]
    pub fn memory_id(&self) -> &str {
        &self.memory_id
    }

    /// Returns the span the summary covers.
    #[must_use]
    pub const fn span(&self) -> SummarySpan {
        self.span
    }

    /// Returns what the summary compressed.
    #[must_use]
    pub const fn loss(&self) -> SummaryLoss {
        self.loss
    }

    /// Returns when the summary was recorded.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

/// Records a summary and the span it covers, refusing an overlap with a summary already stored.
///
/// `session_message_count` is the session's current message count, read by the caller. It is passed in rather
/// than read here for a specific reason: `jarvisd` holds the database and the transcript is its own concern,
/// and a caller that read the count first and then called this would be checking a span against a snapshot it
/// no longer holds. Passing it in makes the caller fetch it on the path that is about to write.
///
/// # Errors
///
/// - [`DatabaseError::InvalidSummaryRequest`] when the span names turns the session does not have, or the
///   claimed turn count disagrees with the span's width.
/// - [`DatabaseError::SummaryOverlapsExisting`] when a stored summary covers any of these turns.
/// - [`DatabaseError::MemoryDuplicate`] when a memory already exists for the summary's search key.
/// - [`DatabaseError::StoredMemoryInvalid`] when the summary cannot be expressed as a memory.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn record_summary(
    database: &SqliteDatabase,
    summary: &SessionSummary,
    session_message_count: i64,
) -> Result<StoredSummary, DatabaseError> {
    let span = summary.span();

    // The span is checked against the transcript before anything is written, so a fabricated range never
    // reaches the table. The direction is the one `covers_only_existing_messages` states in its name: the
    // span must be a **subset** of the session's messages.
    if !span.covers_only_existing_messages(session_message_count) {
        return Err(DatabaseError::InvalidSummaryRequest { field: "span" });
    }

    // The loss figures describe the input, so a count that disagrees with the span's width is a producer that
    // read one range and reported another. Refused here rather than reconciled: trusting either one silently
    // would store a compression ratio computed against the wrong denominator.
    let width =
        u32::try_from(span.message_count()).map_err(|_| DatabaseError::InvalidSummaryRequest {
            field: "turns_covered",
        })?;
    if summary.loss().turns_covered != width {
        return Err(DatabaseError::InvalidSummaryRequest {
            field: "turns_covered",
        });
    }

    let record =
        summary
            .clone()
            .into_record()
            .map_err(|error| DatabaseError::StoredMemoryInvalid {
                field: invalid_memory_field(&error),
            })?;
    let key = MemorySearchKey::new(record.memory_type(), record.entities(), record.content())
        .map_err(|error| DatabaseError::StoredMemoryInvalid {
            field: invalid_memory_field(&error),
        })?;

    let mut transaction =
        database
            .pool()
            .begin()
            .await
            .map_err(|source| DatabaseError::Sqlite {
                operation: "begin a summary write",
                source,
            })?;

    // The overlap check, on this connection. `spans_overlap` is `jarvis-core`'s relation, applied here to the
    // rows this query brings back rather than transcribed into a `WHERE` clause: the rule is "same session and
    // the intervals intersect", and a SQL version would be a second definition that can disagree with the one
    // the domain uses.
    let existing = sqlx::query(
        "SELECT first_sequence, last_sequence FROM session_summaries WHERE session_id = ?1",
    )
    .bind(summary.session_id().to_string())
    .fetch_all(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session's summaries",
        source,
    })?;
    for row in &existing {
        let stored = decode_span(row, summary.session_id())?;
        if spans_overlap(stored, span) {
            commit(transaction, "a summary write").await?;
            return Err(DatabaseError::SummaryOverlapsExisting {
                first_sequence: stored.first_sequence,
                last_sequence: stored.last_sequence,
            });
        }
    }

    // The memory half, sharing `record_memory`'s statement rather than listing twenty-six columns a second
    // time. A column omitted from a second copy is not a compile error; it is a memory stored with that field
    // defaulted, and a defaulted `source_trust` would change whether its content may instruct.
    let affected = insert_memory_on(&mut *transaction, &record, &key).await?;
    if affected == 0 {
        let existing_id = find_key_on(&mut transaction, &record, key.as_str()).await?;
        commit(transaction, "a summary write").await?;
        return Err(DatabaseError::MemoryDuplicate {
            existing_memory_id: existing_id.unwrap_or_default(),
        });
    }

    link_entities_on(&mut transaction, &record).await?;

    sqlx::query(
        "INSERT INTO session_summaries (\
            memory_id, session_id, first_sequence, last_sequence, turns_covered, source_chars\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(record.id().to_string())
    .bind(summary.session_id().to_string())
    .bind(span.first_sequence)
    .bind(span.last_sequence)
    .bind(i64::from(summary.loss().turns_covered))
    .bind(
        summary
            .loss()
            .source_chars
            .map(|chars| i64::try_from(chars).unwrap_or(i64::MAX)),
    )
    .execute(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a summary's span",
        source,
    })?;

    commit(transaction, "a summary write").await?;

    Ok(StoredSummary {
        memory_id: record.id().to_string(),
        span,
        loss: summary.loss(),
        created_at: record.created_at(),
    })
}

/// Maximum summaries one read may return.
pub const MAX_SUMMARY_PAGE: u32 = 128;

/// Reads a session's **current** summaries, largest span first.
///
/// # Why this filters on `active` and the span read does not
///
/// The two reads answer different questions and the difference is deliberate. This one answers "what does this
/// session's summary currently say", so it excludes a summary that was deleted **and** one that retention
/// archived — `MemoryStatus::Archived` is "retained for audit, not retrieved as current truth", and a read that
/// returned an archived summary would present a set-aside claim as the current one.
///
/// [`read_session_summary_spans`] does not filter, because the overlap check must still see a set-aside span:
/// otherwise archiving or removing a summary would re-open its turns to a second claim about the same passage.
/// An audit or export path that wants archived summaries reads them through the memory read, which includes
/// archived rows — not through this one.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidSummaryRequest`] when `limit` is out of range,
/// [`DatabaseError::StoredSummaryInvalid`] when a stored row names a span the domain refuses, and
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_session_summaries(
    database: &SqliteDatabase,
    session_id: SessionId,
    limit: u32,
) -> Result<Vec<StoredSummary>, DatabaseError> {
    if limit == 0 || limit > MAX_SUMMARY_PAGE {
        return Err(DatabaseError::InvalidSummaryRequest { field: "limit" });
    }
    let rows = sqlx::query(
        "SELECT s.memory_id, s.first_sequence, s.last_sequence, s.turns_covered, s.source_chars, \
                m.created_at \
         FROM session_summaries s JOIN memories m ON m.id = s.memory_id \
         WHERE s.session_id = ?1 AND m.status = 'active' \
         ORDER BY s.last_sequence DESC, s.memory_id DESC LIMIT ?2",
    )
    .bind(session_id.to_string())
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session's summaries",
        source,
    })?;

    rows.iter()
        .map(|row| decode_summary(row, session_id))
        .collect()
}

/// Reads the spans of a session's summaries, lowest first.
///
/// This is the input [`jarvis_core::is_offerable_as_context`] takes: a caller assembling a prompt supplies the
/// spans it is **replaying** and asks whether a summary may be offered. Exposed as its own read rather than
/// left to [`read_session_summaries`] so a caller that wants only the spans does not decode loss metadata it
/// will not use — and so "what has already been covered" is not a query that also returns text.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSummaryInvalid`] when a stored row names a span the domain refuses, and
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_session_summary_spans(
    database: &SqliteDatabase,
    session_id: SessionId,
) -> Result<Vec<SummarySpan>, DatabaseError> {
    let rows = sqlx::query(
        "SELECT first_sequence, last_sequence FROM session_summaries WHERE session_id = ?1 \
         ORDER BY first_sequence ASC",
    )
    .bind(session_id.to_string())
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session's summary spans",
        source,
    })?;

    rows.iter()
        .map(|row| decode_span(row, session_id))
        .collect()
}

/// Reads the message ranges a session's summaries **do not** cover, oldest first.
///
/// # Why the caller wants the complement rather than the covered spans
///
/// "Is this session summarized" is the wrong question: a long session is summarized in pieces, and a consumer
/// deciding whether to compress more needs the turns that **remain**, not the union of what is done. Returning
/// the complement means the caller does not derive it by subtracting two sorted lists, which is the arithmetic
/// that quietly omits the range between two adjacent covered spans.
///
/// It is also the answer to whether the retention rule is meaningful yet. An empty complement means the
/// transcript is fully represented by summaries — the state in which offering a summary stops being a
/// duplicate — and a complement of the whole transcript means nothing has been compressed.
///
/// # Errors
///
/// Returns [`DatabaseError::StoredSummaryInvalid`] when a stored row names a span the domain refuses, and
/// [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_unsummarized_ranges(
    database: &SqliteDatabase,
    session_id: SessionId,
    message_count: i64,
) -> Result<Vec<SummarySpan>, DatabaseError> {
    if message_count <= 0 {
        return Ok(Vec::new());
    }
    let mut covered = read_session_summary_spans(database, session_id).await?;
    covered.sort_by_key(|span| span.first_sequence);

    let mut gaps = Vec::new();
    let mut next = 0_i64;
    for span in covered {
        if span.first_sequence > next {
            gaps.push(
                SummarySpan::new(session_id, next, span.first_sequence - 1)
                    .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "gap" })?,
            );
        }
        // `max` rather than assignment: two stored spans can overlap if they were written by another build,
        // and a later span that ends earlier must not move the cursor backwards.
        next = next.max(span.last_sequence + 1);
    }
    if next < message_count {
        gaps.push(
            SummarySpan::new(session_id, next, message_count - 1)
                .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "gap" })?,
        );
    }
    Ok(gaps)
}

/// Which session memories a retention sweep may archive, as a SQL predicate.
///
/// # Why this is derived from `MemoryType::is_durable` rather than written out
///
/// `docs/architecture/memory-and-context.md` says `Conversation` is the type whose "session retention applies",
/// and `MemoryType::is_durable` already states which types those are: `Working` and `Conversation`. Writing the
/// list again here as `memory_type IN ('working', 'conversation')` would be a **second definition of the same
/// rule**, and the two would drift the first time a type is added — the drift being that a new session-scoped
/// type is never collected, or a durable one is archived by a sweep. Generating the `IN` list from the
/// predicate makes the schema's filter and the domain's rule the same fact.
///
/// `pub(crate)` because it is a query fragment: it is not a safe string to build a statement with, and the one
/// caller below is the statement that uses it.
pub(crate) fn collection_predicate() -> String {
    let names: Vec<String> = jarvis_core::MemoryType::all()
        .iter()
        .filter(|memory_type| !memory_type.is_durable())
        .map(|memory_type| format!("'{}'", memory_type.as_str()))
        .collect();
    format!("memory_type IN ({})", names.join(", "))
}

/// Archives every summary of one session, returning how many rows changed.
///
/// # The retention rule, and why a summary has one
///
/// "Session retention applies" is the documented rule for a `Conversation` memory, and `P4-015` requires the
/// summary to carry one. The rule is not a timer: a summary is archived when the **session** it belongs to is
/// archived, because the session is the unit whose retention the rule names. Nothing expires on a clock here,
/// which is consistent with `P4-008`'s recorded limit ("no retention policy is implemented — the verbs exist
/// and the sweeper does not").
///
/// # Why this is archive and not delete
///
/// `MemoryStatus::Archived` is "retained for audit, not retrieved as current truth" — which is exactly the
/// outcome wanted: a session's summary stops being offered, and the text is still there for the export and for a
/// user who changes their mind. Deleting would also have to clear the search key and write a tombstone, turning
/// a retention decision into a **forget**, which is a different verb with a different receipt.
///
/// # Why it does not filter on the session's own status
///
/// The caller decides *when*; this decides *what*. A statement that also read `sessions.status` would refuse to
/// run after a hand-edit archived the session without setting the timestamp, and its count would silently be
/// zero — the failure shape where a sweep reports success and did nothing.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the update fails.
pub async fn archive_session_summaries(
    database: &SqliteDatabase,
    session_id: SessionId,
    at: UtcTimestamp,
) -> Result<u64, DatabaseError> {
    // Both halves in one statement: the join restricts to this session's summaries, and the predicate from
    // `collection_predicate` is what makes "this is a session-scoped record" a rule rather than a comment.
    let sql = format!(
        "UPDATE memories SET status = 'archived', updated_at = ?2, version = version + 1 \
         WHERE id IN (SELECT memory_id FROM session_summaries WHERE session_id = ?1) \
           AND status = 'active' AND {}",
        collection_predicate()
    );
    let result = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(session_id.to_string())
        .bind(at.to_string())
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "archive a session's summaries",
            source,
        })?;
    Ok(result.rows_affected())
}

/// Commits a transaction, naming the operation for the error.
async fn commit(
    transaction: sqlx::Transaction<'_, sqlx::Sqlite>,
    operation: &'static str,
) -> Result<(), DatabaseError> {
    transaction
        .commit()
        .await
        .map_err(|source| DatabaseError::Sqlite { operation, source })
}

/// Writes the memory's entity links on this connection.
async fn link_entities_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    record: &MemoryRecord,
) -> Result<(), DatabaseError> {
    for entity in record.entities() {
        sqlx::query(
            "INSERT INTO memory_entities (memory_id, entity_id, matched_by) VALUES (?1, ?2, ?3) \
             ON CONFLICT (memory_id, entity_id) DO UPDATE SET matched_by = excluded.matched_by",
        )
        .bind(record.id().to_string())
        .bind(entity.entity_id().to_string())
        .bind(entity.matched_by().as_str())
        .execute(&mut **transaction)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "link a summary to its subject",
            source,
        })?;
    }
    Ok(())
}

/// Finds the memory already recorded for this search key, on this connection.
async fn find_key_on(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    record: &MemoryRecord,
    search_key: &str,
) -> Result<Option<String>, DatabaseError> {
    let row = sqlx::query("SELECT id FROM memories WHERE workspace_id = ?1 AND search_key = ?2")
        .bind(record.workspace_id().to_string())
        .bind(search_key)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "find a memory by its search key",
            source,
        })?;
    row.map(|row| {
        row.try_get::<String, _>("id")
            .map_err(|_| DatabaseError::StoredMemoryInvalid { field: "id" })
    })
    .transpose()
}

fn decode_span(
    row: &sqlx::sqlite::SqliteRow,
    session_id: SessionId,
) -> Result<SummarySpan, DatabaseError> {
    let sequence = |field: &'static str| -> Result<i64, DatabaseError> {
        row.try_get(field)
            .map_err(|_| DatabaseError::StoredSummaryInvalid { field })
    };
    SummarySpan::new(
        session_id,
        sequence("first_sequence")?,
        sequence("last_sequence")?,
    )
    .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "span" })
}

fn decode_summary(
    row: &sqlx::sqlite::SqliteRow,
    session_id: SessionId,
) -> Result<StoredSummary, DatabaseError> {
    let span = decode_span(row, session_id)?;
    let turns_covered = row
        .try_get::<i64, _>("turns_covered")
        .ok()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(DatabaseError::StoredSummaryInvalid {
            field: "turns_covered",
        })?;
    let source_chars = row
        .try_get::<Option<i64>, _>("source_chars")
        .map_err(|_| DatabaseError::StoredSummaryInvalid {
            field: "source_chars",
        })?
        .map(|chars| {
            usize::try_from(chars).map_err(|_| DatabaseError::StoredSummaryInvalid {
                field: "source_chars",
            })
        })
        .transpose()?;
    let loss = SummaryLoss::new(turns_covered, source_chars)
        .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "loss" })?;
    let created_at = row.try_get::<String, _>("created_at").map_err(|_| {
        DatabaseError::StoredSummaryInvalid {
            field: "created_at",
        }
    })?;
    Ok(StoredSummary {
        memory_id: row
            .try_get::<String, _>("memory_id")
            .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "memory_id" })?,
        span,
        loss,
        created_at: crate::memory_repository::parse_timestamp(&created_at, "created_at")?,
    })
}

#[cfg(test)]
#[path = "summary_tests.rs"]
mod tests;
