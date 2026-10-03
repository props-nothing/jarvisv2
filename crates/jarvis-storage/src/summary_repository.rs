//! Durable storage for session summaries (`P4-015`).
//!
//! A summary is recorded as a `memories` row and, separately, a `session_summaries` row naming the turns it
//! compressed. `into_record` produces the memory; this adapter writes both halves **in one transaction** and
//! answers the one question the writer cannot answer alone.
//!
//! # The overlap check is read-then-write, and that is why it is one transaction
//!
//! The write refuses to record a summary of turns another summary already covers. That check is a `SELECT`
//! over `session_summaries` followed by two inserts, and a deferred transaction that reads and then writes
//! fails with `SQLITE_BUSY_SNAPSHOT` if another connection commits in between — which is the failure
//! `purge_memory` records in its own note, found the same way. So the check and both writes run on **one**
//! connection, through this one function, and a caller cannot split them.
//!
//! # What this module deliberately does not do
//!
//! It does not compose the summary text. Compression is a model's job and the *quality* of the text is not
//! something a schema can judge; what the schema and this adapter enforce is that whatever text arrives is
//! **attributed**: `Conversation` type, `Document` source kind (so `Derived` trust), a locator naming the
//! session and the exact span, and the span recorded against the transcript it claims. That is the half of
//! "is never presented as user-authored fact" that is checkable.
//!
//! It also does not delete a summary whose span a newer summary subsumes. Supersession is a status
//! transition on the memory (`MemoryRecord::supersede`), and the overlap check refuses to *create* the
//! overlap in the first place — which is the cheaper moment to notice it.

use jarvis_core::{
    CurrentClock, MemorySearchKey, SessionId, SessionSummary, SummaryLoss, SummarySpan, UtcTimestamp,
    spans_overlap,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// One stored summary, with the span and loss that make it a summary.
///
/// # Why the span and loss are returned and not only the memory id
///
/// They are what a caller needs to decide anything about a summary and what no `memories` column holds. A
/// consumer that had to reconstruct the span from the source locator would be parsing a string this module
/// formats, which is a contract kept in two places and broken by editing either.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredSummary {
    memory_id: String,
    session_id: String,
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
    pub const fn loss(&self) -> &SummaryLoss {
        &self.loss
    }

    /// Returns when the summary was recorded.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

/// Records a summary and the span it covers, refusing an overlap with a summary already stored.
///
/// `session_message_count` is the session's current message count, read by the caller — `jarvisd` holds the
/// database and the transcript is its own concern. It is passed in rather than read here so the check happens
/// on the **same connection and inside the same transaction** as the write: a caller that read the count
/// first and then opened a transaction would be checking against a snapshot it no longer holds.
///
/// # Errors
///
/// - [`DatabaseError::SummaryOverlapsExisting`] when a stored summary covers any of these turns.
/// - [`DatabaseError::InvalidSummaryRequest`] when the span names turns the session does not have, or the
///   claimed turn count disagrees with the span.
/// - [`DatabaseError::MemoryDuplicate`] when a memory already exists for the summary's search key.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn record_summary(
    database: &SqliteDatabase,
    summary: &SessionSummary,
    actor_name: &str,
) -> Result<StoredSummary, DatabaseError> {
    // The span is checked against the transcript before anything is written, so a fabricated range never
    // reaches the table. `covers_only_existing_messages` states the direction: the span must be a subset.
    let message_count = crate::message_repository::count_messages(database, &summary.session_id().to_string())
        .await
        .map_err(|source| match source {
            DatabaseError::SessionNotFound => DatabaseError::SessionNotFound,
            other => other,
        })?;
    let span = summary.span();
    if !span.covers_only_existing_messages(message_count) {
        return Err(DatabaseError::InvalidSummaryRequest {
            field: "span",
        });
    }
    if summary.loss().turns_covered() != u32::try_from(span.message_count()).unwrap_or(u32::MAX) {
        return Err(DatabaseError::InvalidSummaryRequest {
            field: "turns_covered",
        });
    }

    let record = summary.into_record()?;
    let key = MemorySearchKey::from(actor_name, record.content());

    let mut transaction = database.pool().begin().await.map_err(|source| {
        DatabaseError::Sqlite {
            operation: "begin a summary write",
            source,
        }
    })?;

    // The overlap check, on this connection. `spans_overlap` is `jarvis-core`'s rule, applied here to rows
    // this query brings back rather than restated as a `WHERE` clause: the relation is
    // session-equality plus interval intersection, and a SQL transcription of it would be a second
    // definition that can disagree with the one the domain uses.
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
        let stored = SummarySpan::new(
            summary.session_id(),
            read_sequence(row, "first_sequence")?,
            read_sequence(row, "last_sequence")?,
        )?;
        if spans_overlap(stored, span) {
            transaction.commit().await.map_err(|source| DatabaseError::Sqlite {
                operation: "commit a summary write",
                source,
            })?;
            return Err(DatabaseError::SummaryOverlapsExisting {
                first_sequence: stored.first_sequence(),
                last_sequence: stored.last_sequence(),
            });
        }
    }

    // The memory half. The tombstone check and the duplicate resolution are deliberately **not** here: a
    // tombstone for a summary's search key means the user removed a summary of these same words, and the
    // resurrection rule is `record_memory`'s. A summary is not re-ingested from a source, so this write does
    // not consult them — recorded as a limit in `docs/data/schema.md`.
    let affected = insert_memory_in(&mut transaction, &record, &key).await?;
    if affected == 0 {
        transaction.commit().await.map_err(|source| DatabaseError::Sqlite {
            operation: "commit a summary write",
            source,
        })?;
        let existing_id =
            find_memory_id_by_key_in(&mut transaction, record.workspace_id(), key.as_str())
                .await
                .unwrap_or(None);
        return Err(DatabaseError::MemoryDuplicate {
            existing_memory_id: existing_id.unwrap_or_default(),
        });
    }

    for entity in record.entities() {
        sqlx::query(
            "INSERT INTO memory_entities (memory_id, entity_id, matched_by) VALUES (?1, ?2, ?3) \
             ON CONFLICT (memory_id, entity_id) DO UPDATE SET matched_by = excluded.matched_by",
        )
        .bind(record.id().to_string())
        .bind(entity.entity_id().to_string())
        .bind(entity.matched_by().as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "link a summary to its subject",
            source,
        })?;
    }

    sqlx::query(
        "INSERT INTO session_summaries (\
            memory_id, session_id, first_sequence, last_sequence, turns_covered, source_chars\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(record.id().to_string())
    .bind(summary.session_id().to_string())
    .bind(span.first_sequence())
    .bind(span.last_sequence())
    .bind(i64::from(summary.loss().turns_covered()))
    .bind(summary.loss().source_chars().map(|chars| {
        i64::try_from(chars).unwrap_or(i64::MAX)
    }))
    .execute(&mut *transaction)
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "record a summary's span",
        source,
    })?;

    transaction.commit().await.map_err(|source| DatabaseError::Sqlite {
        operation: "commit a summary write",
        source,
    })?;

    Ok(StoredSummary {
        memory_id: record.id().to_string(),
        session_id: summary.session_id().to_string(),
        span,
        loss: *summary.loss(),
        created_at: record.created_at(),
    })
}

/// Reads a session's summaries, newest span first.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails, and [`DatabaseError::StoredSummaryInvalid`] when a
/// stored row names a span the domain refuses.
pub async fn read_session_summaries(
    database: &SqliteDatabase,
    session_id: SessionId,
    limit: u32,
) -> Result<Vec<StoredSummary>, DatabaseError> {
    if limit == 0 || limit > MAX_SUMMARY_PAGE {
        return Err(DatabaseError::InvalidSummaryRequest { field: "limit" });
    }
    let rows = sqlx::query(
        "SELECT s.memory_id, s.session_id, s.first_sequence, s.last_sequence, s.turns_covered, \
                s.source_chars, m.created_at \
         FROM session_summaries s JOIN memories m ON m.id = s.memory_id \
         WHERE s.session_id = ?1 AND m.status <> 'deleted' \
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

    rows.iter().map(|row| decode_summary(row, session_id)).collect()
}

/// Maximum summaries one read may return.
pub const MAX_SUMMARY_PAGE: u32 = 128;

/// Reads the spans of a session's summaries, for the overlap check a caller runs before writing.
///
/// This is the input [`jarvis_core::is_offerable_as_context`] takes: a caller building a prompt supplies the
/// spans it is **replaying** and asks whether a summary may be offered. Exposed as its own read rather than
/// left to `read_session_summaries` so a caller that wants only the spans does not decode loss metadata it
/// will not use.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
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
        .map(|row| {
            SummarySpan::new(
                session_id,
                read_sequence(row, "first_sequence")?,
                read_sequence(row, "last_sequence")?,
            )
            .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "span" })
        })
        .collect()
}

/// Reads the message ranges a session's summaries **do not** cover, oldest first.
///
/// # Why the caller needs this rather than the covered spans
///
/// "Is this session summarized" is the wrong question: a long session is summarized in pieces, and a
/// consumer deciding whether to compress more needs the turns that remain — not the union of what is done.
/// Returning the complement means the caller does not re-derive it by subtracting two sorted lists, which is
/// the kind of arithmetic that quietly omits the range between two adjacent covered spans.
///
/// It is also the answer to "is the retention rule meaningful yet": with a complement of the whole transcript
/// the summary has compressed nothing, and with an empty complement the transcript is fully represented.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_unsummarized_ranges(
    database: &SqliteDatabase,
    session_id: SessionId,
    message_count: i64,
) -> Result<Vec<SummarySpan>, DatabaseError> {
    if message_count <= 0 {
        return Ok(Vec::new());
    }
    let mut covered = read_session_summary_spans(database, session_id).await?;
    covered.sort_by_key(|span| span.first_sequence());

    let mut gaps = Vec::new();
    let mut next = 0_i64;
    for span in covered {
        if span.first_sequence() > next {
            gaps.push(SummarySpan::new(
                session_id,
                next,
                span.first_sequence() - 1,
            )?);
        }
        next = next.max(span.last_sequence() + 1);
    }
    if next < message_count {
        gaps.push(SummarySpan::new(session_id, next, message_count - 1)?);
    }
    Ok(gaps)
}

fn read_sequence(row: &sqlx::sqlite::SqliteRow, field: &'static str) -> Result<i64, DatabaseError> {
    row.try_get(field)
        .map_err(|_| DatabaseError::StoredSummaryInvalid { field })
}

fn decode_summary(
    row: &sqlx::sqlite::SqliteRow,
    session_id: SessionId,
) -> Result<StoredSummary, DatabaseError> {
    let fields = |field: &'static str| -> Result<String, DatabaseError> {
        row.try_get(field)
            .map_err(|_| DatabaseError::StoredSummaryInvalid { field })
    };
    let span = SummarySpan::new(
        session_id,
        read_sequence(row, "first_sequence")?,
        read_sequence(row, "last_sequence")?,
    )
    .map_err(|_| DatabaseError::StoredSummaryInvalid { field: "span" })?;
    let turns_covered = row
        .try_get::<i64, _>("turns_covered")
        .map_err(|_| DatabaseError::StoredSummaryInvalid {
            field: "turns_covered",
        })?;
    let turns_covered =
        u32::try_from(turns_covered).map_err(|_| DatabaseError::StoredSummaryInvalid {
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
    let created_at = fields("created_at")?;
    Ok(StoredSummary {
        memory_id: fields("memory_id")?,
        session_id: session_id.to_string(),
        span,
        loss,
        created_at: crate::memory_repository::parse_timestamp(&created_at, "created_at")?,
    })
}

/// Re-exported so this module's callers do not need the memory repository for the write it delegates.
use crate::memory_repository::{find_memory_id_by_key_in, insert_memory_in};

/// The clock a caller may use when it has none of its own, so a call site cannot forget one.
#[must_use]
pub fn system_clock() -> CurrentClock {
    CurrentClock
}
