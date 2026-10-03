-- Session summaries: which turns a summary compressed, and how much it compressed them (`P4-015`).
--
-- A summary is recorded as a `memories` row — `Conversation` type, `Document` source kind, therefore
-- `Derived` trust — because it *is* a memory of a conversation and every rule that applies to a memory
-- (workspace scoping, entity links, tombstones, supersession, sensitivity) must apply to it too. What a
-- memory row cannot say is the part that makes a summary a summary: **which turns it covers**.
--
-- # Why the span is a table and not a column on `memories`
--
-- This is the question `ADR-0117` §4 answered the other way for a skill's approver, so the reasoning has to
-- be made explicit rather than copied. A summary's span is not a property of *being a memory*; it is a
-- property of *being a summary*, and it is asked by a different reader:
--
--   * It is **not** read on the retrieval path. Retrieval decides whether to offer a summary; it never needs
--     the sequence range to do so.
--   * It **is** read by the writer, to answer "has this session already been summarized here" before
--     recording a second summary of the same turns. `spans_overlap` in `jarvis-core` is that rule.
--
-- A nullable column pair on every memory row would therefore be two columns that are meaningless for all
-- seven memory types except one, which is how a schema acquires fields nobody can interpret. A table whose
-- primary key *is* the memory id makes "this row is a summary" and "this row has a span" the same fact.
--
-- # Why the `loss` figures are here and not derivable
--
-- `turns_covered` and `source_chars` describe the input, and the input is the transcript — which **changes**.
-- Deriving them at read time would recompute them against a session whose later messages have arrived, so a
-- summary of messages 1..4 would report a compression ratio against messages 1..400. The figures are stored
-- as what they measured at the moment of writing, which is the only moment at which they are true. This is
-- also why `source_chars` is nullable: a caller that did not measure its input must say so rather than
-- report `0`, which would be a ratio of infinity dressed as a measurement.
--
-- # Why `source_chars = 0` is refused rather than merely allowed to yield no ratio
--
-- A zero-byte input cannot produce a summary that covers a turn — the turn had content. The domain refuses
-- it (`SummaryLoss::new`), and the `CHECK` restates it here, because a stored `0` would be a row claiming to
-- have compressed nothing into something.
--
-- # Why the bounds are length checks rather than value checks
--
-- The span is two sequence numbers, and the schema cannot know the session's message count — that is a
-- fact about a different table, evaluated at a different time. So the columns carry only the bounds SQLite
-- can state (`first >= 0`, matching `messages.sequence`, and `last >= first`), and the rule that the span
-- lies inside the session is enforced where the transcript is: in the writer, which reads the session's
-- message count on the same connection.

CREATE TABLE session_summaries (
    memory_id TEXT PRIMARY KEY NOT NULL REFERENCES memories (id) ON DELETE CASCADE,
    session_id TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    -- Matches `messages.sequence`, which the transcript's own `CHECK` holds at `>= 0`.
    first_sequence INTEGER NOT NULL CHECK (first_sequence >= 0),
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= first_sequence),
    -- The number of messages the summary covered, as measured when it was written. Bounded above by the
    -- span's own width in the domain rather than here, because `MAX_SUMMARIZED_TURNS` is a `jarvis-core`
    -- constant and restating its value would be a second place for it to live.
    turns_covered INTEGER NOT NULL CHECK (turns_covered >= 1),
    -- Nullable, and the null is meaningful: the input size was not measured. See the note above on why a
    -- `0` is not the way to say that.
    source_chars INTEGER CHECK (source_chars IS NULL OR source_chars >= 1)
) STRICT;

-- The writer's question is "which summaries already cover part of this session", so the index is on the
-- session and leads with the range. Without it that check is a full scan of the table for every write.
CREATE INDEX session_summaries_session_range_idx
    ON session_summaries (session_id, first_sequence, last_sequence);

-- The read that follows a summary is "what does this session's summary say", keyed by session and newest
-- first, which is the same index read backwards. A separate descending index would be a second structure
-- for the same rows.

UPDATE jarvis_storage_metadata
SET schema_version = 12
WHERE singleton = 1;

PRAGMA user_version = 12;
