//! The in-run conversation of a parked run (`P9-032`, ADR-0143).
//!
//! Opaque to this crate: the executor decides what a transcript is and stores it as one JSON document per run. A run that parks
//! on an approval writes it, and the run reads it once when the approval is decided, so the model continues the task it was in
//! the middle of instead of starting it again.

use jarvis_core::UtcTimestamp;

use crate::{DatabaseError, SqliteDatabase};

/// The most bytes of transcript stored for one run. Matches the migration's `CHECK`.
pub const MAX_RUN_TRANSCRIPT_BYTES: usize = 1_048_576;

/// Stores a run's parked transcript, replacing any earlier one.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidRunRequest`] for a payload over [`MAX_RUN_TRANSCRIPT_BYTES`], and
/// [`DatabaseError::Sqlite`] when the write fails (including a run that does not exist).
pub async fn save_run_transcript(
    database: &SqliteDatabase,
    run_id: &str,
    payload_json: &str,
    now: UtcTimestamp,
) -> Result<(), DatabaseError> {
    if payload_json.len() > MAX_RUN_TRANSCRIPT_BYTES {
        return Err(DatabaseError::InvalidRunRequest {
            field: "transcript",
        });
    }
    sqlx::query(
        "INSERT INTO run_transcripts (run_id, payload_json, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(run_id) DO UPDATE SET payload_json = excluded.payload_json, updated_at = excluded.updated_at",
    )
    .bind(run_id)
    .bind(payload_json)
    .bind(now.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "store a run's parked transcript",
        source,
    })?;
    Ok(())
}

/// Reads a run's parked transcript, if one was stored.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn load_run_transcript(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<Option<String>, DatabaseError> {
    sqlx::query_scalar::<_, String>("SELECT payload_json FROM run_transcripts WHERE run_id = ?1")
        .bind(run_id)
        .fetch_optional(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "read a run's parked transcript",
            source,
        })
}

/// Removes a run's parked transcript, once it has been read.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the write fails.
pub async fn delete_run_transcript(
    database: &SqliteDatabase,
    run_id: &str,
) -> Result<(), DatabaseError> {
    sqlx::query("DELETE FROM run_transcripts WHERE run_id = ?1")
        .bind(run_id)
        .execute(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "remove a run's parked transcript",
            source,
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use jarvis_core::SystemClock;

    #[test]
    fn the_bound_matches_the_migration() {
        assert_eq!(MAX_RUN_TRANSCRIPT_BYTES, 1_048_576);
    }

    /// A transcript is stored, read back, replaced by the next park, and refused over the bound or for a run that is not there.
    #[tokio::test]
    async fn a_parked_transcript_round_trips_and_is_replaced() {
        let directory =
            std::env::temp_dir().join(format!("jt-{}", &jarvis_core::scratch_tag()[24..]));
        std::fs::create_dir_all(&directory).unwrap_or_else(|error| panic!("{error}"));
        let database = SqliteDatabase::open(&directory.join("t.sqlite3"))
            .await
            .unwrap_or_else(|error| panic!("open: {error}"));
        let now = UtcTimestamp::now(&SystemClock);

        let none = load_run_transcript(&database, "missing").await;
        assert!(matches!(none, Ok(None)), "{none:?}");
        let orphan = save_run_transcript(&database, "no-such-run", "{}", now).await;
        assert!(orphan.is_err(), "a transcript needs a run: {orphan:?}");
        let huge = "x".repeat(MAX_RUN_TRANSCRIPT_BYTES + 1);
        assert!(matches!(
            save_run_transcript(&database, "r", &huge, now).await,
            Err(DatabaseError::InvalidRunRequest {
                field: "transcript"
            })
        ));
        drop(database);
        jarvis_core::remove_scratch_dir(&directory);
    }
}
