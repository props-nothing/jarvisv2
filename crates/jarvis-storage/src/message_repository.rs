//! Durable storage for conversation messages.
//!
//! `P2-004` created the `messages` table; this adapter is the first production writer. Until it
//! existed only test fixtures inserted rows, so an accepted user message was **not** recorded —
//! which is what `docs/quality/acceptance-tests.md` A03 requires ("never loses an accepted user
//! message").
//!
//! # Sequence is allocated by the writing statement
//!
//! `UNIQUE (session_id, sequence)` means two writers computing "the next sequence" from a prior read
//! can collide. The allocation and the insert are therefore one `INSERT ... SELECT`, for the reason
//! the run-event repository records: a separate read takes a snapshot another writer can invalidate,
//! and in WAL mode the upgrade to a write then fails with `SQLITE_BUSY_SNAPSHOT` rather than waiting.

use jarvis_core::{
    CorrelationId, MAX_MESSAGE_CONTENT_BYTES, MessageRole, MessageSource, NewMessage, UtcTimestamp,
};
use sqlx::Row;

use crate::database::{DatabaseError, SqliteDatabase};

/// One stored conversation message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredMessage {
    id: String,
    session_id: String,
    sequence: i64,
    role: MessageRole,
    source: MessageSource,
    content: String,
    run_id: Option<String>,
    created_at: UtcTimestamp,
}

impl StoredMessage {
    /// Returns the message identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the session the message belongs to.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Returns this message's position in its session, from zero.
    #[must_use]
    pub const fn sequence(&self) -> i64 {
        self.sequence
    }

    /// Returns the author role.
    #[must_use]
    pub const fn role(&self) -> MessageRole {
        self.role
    }

    /// Returns where the content originated.
    #[must_use]
    pub const fn source(&self) -> MessageSource {
        self.source
    }

    /// Returns the content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns the run that produced this message, when one did.
    ///
    /// Absent for a user message, which is stored before the run it triggers exists, and set once it
    /// does. This is what lets a transcript distinguish "the user asked this" from "the run answered
    /// it" without a second ordering column.
    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    /// Returns when the message was recorded.
    #[must_use]
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

/// Appends one message to a session's transcript.
///
/// # Errors
///
/// - [`DatabaseError::SessionNotFound`] when no session has that identifier.
/// - [`DatabaseError::InvalidMessageRequest`] when the content violates a schema bound.
/// - [`DatabaseError::Sqlite`] for any other persistence failure.
pub async fn append_message(
    database: &SqliteDatabase,
    session_id: &str,
    run_id: Option<&str>,
    message: &NewMessage,
    correlation_id: CorrelationId,
    now: UtcTimestamp,
) -> Result<StoredMessage, DatabaseError> {
    if message.content().len() > MAX_MESSAGE_CONTENT_BYTES {
        return Err(DatabaseError::InvalidMessageRequest { field: "content" });
    }
    let id = jarvis_core::SessionId::new().to_string();

    // The `WHERE EXISTS` clause is the session guard, folded into the insert so the guard and the
    // write are one statement. A session that does not exist affects zero rows, and the reason is
    // resolved on the failure path rather than guessed at here.
    let result = sqlx::query(
        "INSERT INTO messages (\
            id, session_id, sequence, author_kind, role, content, content_bytes, \
            sensitivity, source, run_id, created_at\
         ) \
         SELECT ?1, ?2, \
            (SELECT COALESCE(MAX(sequence) + 1, 0) FROM messages WHERE session_id = ?2), \
            ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10 \
         WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?2)",
    )
    .bind(&id)
    .bind(session_id)
    // `author_kind` and `role` hold the same value because the schema keeps "who spoke" and "where
    // it came from" as separate columns whose value sets happen to coincide for every message this
    // build writes. Both are bound from one validated value rather than two literals that could
    // drift.
    .bind(message.role().as_str())
    .bind(message.role().as_str())
    .bind(message.content())
    .bind(message.content_bytes())
    .bind(message.sensitivity().as_str())
    .bind(message.source().as_str())
    .bind(run_id)
    .bind(now.to_string())
    .execute(database.pool())
    .await
    .map_err(|source| map_message_error(source, &correlation_id))?;

    if result.rows_affected() == 0 {
        return Err(DatabaseError::SessionNotFound);
    }
    find_message(database, &id).await
}

/// Reads one message by identifier.
///
/// # Errors
///
/// Returns [`DatabaseError::MessageNotFound`] when no message has that identifier.
pub async fn find_message(
    database: &SqliteDatabase,
    id: &str,
) -> Result<StoredMessage, DatabaseError> {
    let row = sqlx::query(
        "SELECT id, session_id, sequence, role, source, content, run_id, created_at \
         FROM messages WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a message",
        source,
    })?
    .ok_or(DatabaseError::MessageNotFound)?;
    decode_message(&row)
}

/// Reads a session's transcript in order, bounded by `limit`.
///
/// A **bounded** page rather than the whole transcript: an unbounded read is something a client can
/// trigger for free, and a transcript grows without limit over a long session.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidMessageRequest`] when `limit` is zero or above the maximum page,
/// and [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_messages(
    database: &SqliteDatabase,
    session_id: &str,
    limit: u32,
) -> Result<Vec<StoredMessage>, DatabaseError> {
    if limit == 0 || limit > MAX_MESSAGE_PAGE {
        return Err(DatabaseError::InvalidMessageRequest { field: "limit" });
    }
    let rows = sqlx::query(
        "SELECT id, session_id, sequence, role, source, content, run_id, created_at \
         FROM messages WHERE session_id = ?1 ORDER BY sequence ASC LIMIT ?2",
    )
    .bind(session_id)
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session transcript",
        source,
    })?;

    rows.iter().map(decode_message).collect()
}

/// Maximum messages one read may return.
pub const MAX_MESSAGE_PAGE: u32 = 512;

/// Reads the **most recent** messages of a session, oldest-first among those returned.
///
/// # Why this is not `read_messages(..).rev()`
///
/// A conversation grows without bound while the context that can be replayed into a model call does
/// not. Selecting "the first N" and then taking the last of them would read the *oldest* N and
/// discard the turns closest to the current one, which are the ones a follow-up question depends on.
/// The window therefore has to be chosen by the query, from the end, and then ordered forwards so the
/// transcript replays in the order it happened.
///
/// # Errors
///
/// Returns [`DatabaseError::InvalidMessageRequest`] when `limit` is zero or above the maximum page,
/// and [`DatabaseError::Sqlite`] when the read fails.
pub async fn read_recent_messages(
    database: &SqliteDatabase,
    session_id: &str,
    limit: u32,
) -> Result<Vec<StoredMessage>, DatabaseError> {
    if limit == 0 || limit > MAX_MESSAGE_PAGE {
        return Err(DatabaseError::InvalidMessageRequest { field: "limit" });
    }
    let rows = sqlx::query(
        "SELECT id, session_id, sequence, role, source, content, run_id, created_at \
         FROM (SELECT * FROM messages WHERE session_id = ?1 ORDER BY sequence DESC LIMIT ?2) \
         ORDER BY sequence ASC",
    )
    .bind(session_id)
    .bind(i64::from(limit))
    .fetch_all(database.pool())
    .await
    .map_err(|source| DatabaseError::Sqlite {
        operation: "read a session's recent transcript",
        source,
    })?;

    rows.iter().map(decode_message).collect()
}

/// Counts a session's messages without reading them.
///
/// A caller that windows a transcript needs to know whether older turns exist outside its window.
/// Deriving that from a page length compares against the page bound, which is wrong the moment the
/// bound changes — a count answers the question directly.
///
/// # Errors
///
/// Returns [`DatabaseError::Sqlite`] when the read fails.
pub async fn count_messages(
    database: &SqliteDatabase,
    session_id: &str,
) -> Result<i64, DatabaseError> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE session_id = ?1")
        .bind(session_id)
        .fetch_one(database.pool())
        .await
        .map_err(|source| DatabaseError::Sqlite {
            operation: "count a session transcript",
            source,
        })
}

fn decode_message(row: &sqlx::sqlite::SqliteRow) -> Result<StoredMessage, DatabaseError> {
    let role = text_field(row, "role")?
        .parse::<MessageRole>()
        .map_err(|_| DatabaseError::StoredMessageInvalid { field: "role" })?;
    let source = text_field(row, "source")?
        .parse::<MessageSource>()
        .map_err(|_| DatabaseError::StoredMessageInvalid { field: "source" })?;
    Ok(StoredMessage {
        id: text_field(row, "id")?,
        session_id: text_field(row, "session_id")?,
        sequence: row
            .try_get("sequence")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode a message sequence",
                source,
            })?,
        role,
        source,
        content: text_field(row, "content")?,
        run_id: row
            .try_get("run_id")
            .map_err(|source| DatabaseError::Sqlite {
                operation: "decode a message run id",
                source,
            })?,
        created_at: text_field(row, "created_at")?
            .parse::<UtcTimestamp>()
            .map_err(|_| DatabaseError::StoredMessageInvalid {
                field: "created_at",
            })?,
    })
}

fn text_field(
    row: &sqlx::sqlite::SqliteRow,
    column: &'static str,
) -> Result<String, DatabaseError> {
    row.try_get(column).map_err(|source| DatabaseError::Sqlite {
        operation: "decode a stored message field",
        source,
    })
}

/// Maps a constrained insert failure onto a bounded reason without echoing the row.
fn map_message_error(source: sqlx::Error, _correlation_id: &CorrelationId) -> DatabaseError {
    let text = source.to_string();
    // A foreign-key failure here means the session vanished between the guard and the insert, which
    // a concurrent delete can cause. Reporting it as a missing session keeps the attribution honest
    // rather than surfacing an opaque constraint failure.
    if text.contains("FOREIGN KEY") {
        return DatabaseError::SessionNotFound;
    }
    if text.contains("CHECK") {
        return DatabaseError::InvalidMessageRequest { field: "content" };
    }
    DatabaseError::Sqlite {
        operation: "append a message",
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::database::DEFAULT_DATABASE_FILENAME;

    /// A temporary profile directory holding a migrated database.
    struct TempProfile(PathBuf);

    impl TempProfile {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("jarvis-messages-{}", jarvis_core::scratch_tag()));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create temp profile: {error}"));
            Self(path)
        }

        fn database_path(&self) -> PathBuf {
            self.0.join(DEFAULT_DATABASE_FILENAME)
        }
    }

    impl Drop for TempProfile {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    async fn database() -> (TempProfile, SqliteDatabase) {
        let profile = TempProfile::new();
        let database = SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        (profile, database)
    }

    fn at(minute: i128) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000 + minute * 60_000_000_000)
            .unwrap_or_else(|error| panic!("timestamp: {error}"))
    }

    /// Starts a session through the real start path and returns its identifier.
    async fn session(database: &SqliteDatabase) -> String {
        let identity = crate::load_local_identity(database)
            .await
            .unwrap_or_else(|error| panic!("the fixture must be seeded: {error}"));
        let input = crate::StartRunInput::new(
            jarvis_core::SessionId::new().to_string(),
            jarvis_core::RunId::new().to_string(),
            jarvis_core::RequestId::new().to_string(),
            identity.workspace_id(),
            identity.user_id(),
            "store the transcript",
            crate::API_SESSION_CHANNEL,
            CorrelationId::new(),
            at(0),
        )
        .unwrap_or_else(|error| panic!("start input: {error}"));
        let started = crate::start_run(database, &input)
            .await
            .unwrap_or_else(|error| panic!("start run: {error}"));
        started.session_id().to_owned()
    }

    /// Messages are numbered from zero and in append order, which is what a transcript replay needs.
    #[tokio::test]
    async fn messages_are_sequenced_in_append_order() {
        let (_profile, database) = database().await;
        let session = session(&database).await;

        // The accepted objective is the transcript's first message, because it is written in the
        // same transaction as the run. Asserted here rather than assumed: it is the property that
        // makes an accepted user message impossible to lose, since it exists before the run does.
        let transcript = read_messages(&database, &session, 10)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(
            transcript.len(),
            1,
            "the objective is stored with the run, not after it"
        );
        assert_eq!(transcript[0].sequence(), 0);
        assert_eq!(transcript[0].role(), MessageRole::User);

        for (index, text) in ["first", "second", "third"].iter().enumerate() {
            let message =
                NewMessage::user(*text).unwrap_or_else(|error| panic!("valid message: {error}"));
            let stored = append_message(
                &database,
                &session,
                None,
                &message,
                CorrelationId::new(),
                at(i128::try_from(index).unwrap_or(0) + 1),
            )
            .await
            .unwrap_or_else(|error| panic!("append: {error:?}"));
            assert_eq!(
                stored.sequence(),
                i64::try_from(index).unwrap_or(0) + 1,
                "appends continue from the objective at zero"
            );
        }

        let transcript = read_messages(&database, &session, 10)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        let texts: Vec<&str> = transcript.iter().map(StoredMessage::content).collect();
        assert_eq!(
            texts,
            ["store the transcript", "first", "second", "third"],
            "the transcript keeps append order, objective first"
        );
        database.close().await;
    }

    /// A message for a session that does not exist is refused rather than stored as an orphan.
    #[tokio::test]
    async fn a_message_for_an_unknown_session_is_refused() {
        let (_profile, database) = database().await;
        let message =
            NewMessage::user("orphan").unwrap_or_else(|error| panic!("valid message: {error}"));
        assert!(matches!(
            append_message(
                &database,
                "0198f000-0000-7000-8000-0000000000ff",
                None,
                &message,
                CorrelationId::new(),
                at(1),
            )
            .await,
            Err(DatabaseError::SessionNotFound)
        ));
        database.close().await;
    }

    /// Concurrent appends must not collide on `UNIQUE (session_id, sequence)`. This is the property
    /// the single-statement allocation exists for: a read-then-insert would let two writers compute
    /// the same next sequence.
    #[tokio::test]
    async fn concurrent_appends_do_not_collide_on_sequence() {
        let (_profile, database) = database().await;
        let session = session(&database).await;
        let database = std::sync::Arc::new(database);
        let session_for_tasks = session.clone();

        let mut tasks = Vec::new();
        for index in 0..8 {
            let database = std::sync::Arc::clone(&database);
            let session = session_for_tasks.clone();
            tasks.push(tokio::spawn(async move {
                let message = NewMessage::user(format!("message {index}"))
                    .unwrap_or_else(|error| panic!("valid message: {error}"));
                append_message(
                    &database,
                    &session,
                    None,
                    &message,
                    CorrelationId::new(),
                    at(2),
                )
                .await
            }));
        }

        let mut stored = 0;
        for task in tasks {
            match task.await.unwrap_or_else(|error| panic!("join: {error}")) {
                Ok(_) => stored += 1,
                Err(error) => panic!("a concurrent append must not collide: {error}"),
            }
        }
        assert_eq!(stored, 8);

        let transcript = read_messages(&database, &session, 100)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        // One message already exists — the objective — so the eight appends occupy positions 1..=8.
        let sequences: Vec<i64> = transcript.iter().map(StoredMessage::sequence).collect();
        assert_eq!(
            sequences,
            (0..=8).collect::<Vec<i64>>(),
            "nine messages must occupy nine contiguous positions with no gap or duplicate"
        );
        database.close().await;
    }

    /// An assistant message records the sensitivity it was produced under rather than a default.
    #[tokio::test]
    async fn an_assistant_message_records_its_sensitivity() {
        let (_profile, database) = database().await;
        let session = session(&database).await;
        let message = NewMessage::assistant("the answer", jarvis_core::Sensitivity::Internal)
            .unwrap_or_else(|error| panic!("valid message: {error}"));
        let stored = append_message(
            &database,
            &session,
            None,
            &message,
            CorrelationId::new(),
            at(1),
        )
        .await
        .unwrap_or_else(|error| panic!("append: {error:?}"));
        assert_eq!(stored.role(), MessageRole::Assistant);
        assert_eq!(stored.source(), MessageSource::Model);
        database.close().await;
    }

    /// A page bound of zero or above the maximum is refused rather than silently coerced.
    #[tokio::test]
    async fn an_unusable_page_bound_is_refused() {
        let (_profile, database) = database().await;
        let session = session(&database).await;
        assert!(matches!(
            read_messages(&database, &session, 0).await,
            Err(DatabaseError::InvalidMessageRequest { field: "limit" })
        ));
        assert!(matches!(
            read_messages(&database, &session, MAX_MESSAGE_PAGE + 1).await,
            Err(DatabaseError::InvalidMessageRequest { field: "limit" })
        ));
        assert!(matches!(
            read_recent_messages(&database, &session, 0).await,
            Err(DatabaseError::InvalidMessageRequest { field: "limit" })
        ));
        database.close().await;
    }

    /// A bounded history read selects the **newest** turns and returns them oldest-first.
    ///
    /// This is the property a follow-up question depends on, and the one an obvious implementation
    /// gets wrong: reading the first N and reversing them returns the *oldest* turns, so the turns
    /// closest to the question in hand are the ones dropped.
    #[tokio::test]
    async fn a_bounded_history_read_keeps_the_newest_turns_in_order() {
        let (_profile, database) = database().await;
        let session = session(&database).await;

        for index in 0..9 {
            let message = NewMessage::user(format!("turn {index}"))
                .unwrap_or_else(|error| panic!("valid message: {error}"));
            append_message(
                &database,
                &session,
                None,
                &message,
                CorrelationId::new(),
                at(1),
            )
            .await
            .unwrap_or_else(|error| panic!("append: {error:?}"));
        }

        // Ten messages exist: the objective plus nine turns. A window of four must select the last
        // four, and present them in the order they happened.
        let window = read_recent_messages(&database, &session, 4)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        let contents: Vec<&str> = window.iter().map(StoredMessage::content).collect();
        assert_eq!(
            contents,
            vec!["turn 5", "turn 6", "turn 7", "turn 8"],
            "the window must hold the newest turns, oldest-first among themselves"
        );

        let sequences: Vec<i64> = window.iter().map(StoredMessage::sequence).collect();
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "the returned window must be in ascending sequence order: {sequences:?}"
        );

        // The count is what tells a caller whether turns exist outside the window, which a page
        // length cannot: it is compared against the bound, so it is wrong the moment the bound moves.
        assert_eq!(
            count_messages(&database, &session)
                .await
                .unwrap_or_else(|error| panic!("count: {error}")),
            10
        );
        database.close().await;
    }

    /// A window larger than the transcript returns the whole transcript rather than failing.
    ///
    /// A conversation must not break because a caller asked for more turns than exist. The bound is a
    /// ceiling, not a requirement.
    #[tokio::test]
    async fn a_window_larger_than_the_transcript_returns_all_of_it() {
        let (_profile, database) = database().await;
        let session = session(&database).await;
        let window = read_recent_messages(&database, &session, MAX_MESSAGE_PAGE)
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(window.len(), 1, "only the objective has been stored");
        assert_eq!(window[0].content(), "store the transcript");
        database.close().await;
    }

    /// The count is per session, so one conversation cannot report another's size.
    #[tokio::test]
    async fn the_count_is_scoped_to_its_session() {
        let (_profile, database) = database().await;
        let first = session(&database).await;
        let second = session(&database).await;

        let message = NewMessage::user("only in the second")
            .unwrap_or_else(|error| panic!("valid message: {error}"));
        append_message(
            &database,
            &second,
            None,
            &message,
            CorrelationId::new(),
            at(1),
        )
        .await
        .unwrap_or_else(|error| panic!("append: {error:?}"));

        assert_eq!(
            count_messages(&database, &first)
                .await
                .unwrap_or_else(|error| panic!("count: {error}")),
            1
        );
        assert_eq!(
            count_messages(&database, &second)
                .await
                .unwrap_or_else(|error| panic!("count: {error}")),
            2
        );
        database.close().await;
    }
}
