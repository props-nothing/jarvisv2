-- Scheduled tasks: an objective that becomes a run on a cadence (`P6`, ADR-0132).
--
-- A schedule is a *request to start a run later*, not a second kind of work. When one fires the daemon starts an
-- ordinary run through the ordinary path, so every guarantee a run has -- policy, approvals, audit, the tool
-- pipeline -- applies unchanged, and a scheduled run that wants to fetch a page or run code is held for a person
-- exactly as an interactive one is. Nothing here grants authority.
--
-- # Why two cadences, and why no "daily at 08:00"
--
-- `every` (N seconds) and `once` (one UTC instant) mean the same thing on every host. A wall-clock cadence needs a
-- time zone and daylight-saving rules (`P6-003`), and a half-right version fires a briefing an hour late for half
-- the year without anyone being told. It is recorded as a limit, not approximated.
--
-- # Why the next fire is an integer
--
-- `next_run_nanos` is nanoseconds since the Unix epoch because "is this due?" is a comparison made in SQL, and
-- `jarvis_core::UtcTimestamp`'s text form is not lexicographically sortable (see `0006`). An integer compares
-- correctly; the text columns are for people.
--
-- # One session per schedule
--
-- `session_id` is set when the first fire starts a run and reused afterwards, so a recurring task has a
-- conversation: "what changed since yesterday" has an answer because yesterday's reply is in its history.
--
-- # What a fire does and does not guarantee
--
-- The fire is **claimed** (the next time advanced) in the same guarded `UPDATE` that selects it, *before* the run
-- starts, so a crash between the two loses one fire and never repeats one: at most once. A fire is also skipped
-- when the previous run is still going -- typically parked on an approval -- so an unattended task cannot pile up
-- a queue of identical requests for a person to wade through.

CREATE TABLE schedules (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    objective TEXT NOT NULL CHECK (length(objective) BETWEEN 1 AND 4096),
    cadence TEXT NOT NULL CHECK (cadence IN ('every', 'once')),
    interval_seconds INTEGER CHECK (interval_seconds IS NULL OR interval_seconds BETWEEN 60 AND 31622400),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    next_run_nanos INTEGER NOT NULL,
    session_id TEXT REFERENCES sessions (id) ON DELETE SET NULL,
    last_run_id TEXT,
    last_fired_at TEXT CHECK (last_fired_at IS NULL OR length(last_fired_at) BETWEEN 20 AND 64),
    fire_count INTEGER NOT NULL CHECK (fire_count >= 0),
    skipped_count INTEGER NOT NULL CHECK (skipped_count >= 0),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    -- An interval is present exactly when the cadence repeats: the two columns cannot describe half a schedule.
    CHECK ((cadence = 'every') = (interval_seconds IS NOT NULL))
);

CREATE INDEX schedules_due_idx ON schedules (enabled, next_run_nanos);
CREATE INDEX schedules_workspace_idx ON schedules (workspace_id, created_at);

UPDATE jarvis_storage_metadata
SET schema_version = 14
WHERE singleton = 1;

PRAGMA user_version = 14;
