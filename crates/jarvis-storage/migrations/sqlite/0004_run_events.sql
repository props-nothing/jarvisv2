-- Durable run events: an ordered, replayable record of what a run emitted.
--
-- Why this table exists at all is recorded in ADR-0011. The short version:
-- `docs/architecture/protocols.md` requires stream events to carry a sequence
-- and an event ID and requires the server to "replay from durable records
-- within retention or return an explicit resync requirement". An in-memory
-- counter cannot answer a reconnect after a daemon restart, which is the case
-- the requirement exists for.
--
-- Design notes that are not obvious from the column list:
--
-- * `sequence` is per RUN, not global, and is assigned inside the same
--   transaction as the transition that produced the event. A global sequence
--   would make one run's stream depend on unrelated runs and would leak
--   activity volume across runs to any client that can read two of them.
-- * `UNIQUE (run_id, sequence)` is the ordering guarantee. With it, a duplicate
--   or an out-of-order append cannot be stored, so a retry that would corrupt
--   the stream fails loudly instead of silently renumbering. This is why the
--   insert is a real constraint and not an application convention.
-- * `kind` is a closed set of stable machine-readable names. A client switches
--   on it, so an unknown kind must be rejected by the writer rather than stored
--   and guessed at by a reader. Unknown ADDITIVE FIELDS are tolerated; unknown
--   KINDS are not, matching `docs/architecture/runtime-and-models.md`.
-- * `payload` is bounded JSON for the normalized event body. It is not a copy
--   of the run row: a reader that needs run state reads `agent_runs`.
-- * Hidden chain-of-thought is never stored. `summary` is a bounded operational
--   fact such as "Searching mail", the same rule `agent_steps.summary` carries.
-- * There is no workspace_id column. A run already belongs to exactly one
--   workspace, and duplicating it here would allow the two to disagree. Scoping
--   is enforced by joining through `agent_runs`, which is where workspace
--   filtering already happens.
-- * Retention/compaction is deliberately not implemented here. An unbounded log
--   on a long-lived local install is a real follow-up, tracked separately rather
--   than half-done with a policy nobody measured.

CREATE TABLE run_events (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    run_id TEXT NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'state_changed', 'activity_updated', 'output_delta', 'output_completed',
            'tool_requested', 'approval_requested', 'artifact_created',
            'usage_updated', 'run_completed', 'run_failed', 'run_cancelled', 'heartbeat'
        )
    ),
    -- One direction only: an event is emitted at a moment, and `occurred_at` is
    -- when the run produced it. `recorded_at` is when the row was written, and
    -- the two differ when an event is persisted after a retry. Keeping both
    -- makes "the run emitted this late" distinguishable from "we stored it late".
    occurred_at TEXT NOT NULL CHECK (length(occurred_at) BETWEEN 20 AND 64),
    recorded_at TEXT NOT NULL CHECK (length(recorded_at) BETWEEN 20 AND 64),
    -- Bounded operational fact only. Nullable because a delta carries text and
    -- no summary, while an activity update carries a summary and no text.
    summary TEXT CHECK (summary IS NULL OR length(summary) BETWEEN 1 AND 1024),
    -- The normalized event body. Bounded like every other stored payload so a
    -- pathological event cannot become an unbounded row.
    payload TEXT NOT NULL CHECK (length(CAST(payload AS BLOB)) BETWEEN 2 AND 65536),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    UNIQUE (run_id, sequence)
) STRICT;

-- Replay reads one run in sequence order, and the unique constraint above
-- already provides this index, so no separate index is created for it.
-- This index serves the other access pattern: "what happened recently across
-- runs", which the activity view and diagnostics want.
CREATE INDEX run_events_recorded_idx ON run_events (recorded_at DESC);

UPDATE jarvis_storage_metadata
SET schema_version = 4
WHERE singleton = 1;

PRAGMA user_version = 4;
