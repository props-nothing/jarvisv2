CREATE TABLE daemon_instances (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    process_id INTEGER NOT NULL CHECK (process_id > 0 AND process_id <= 4294967295),
    version TEXT NOT NULL CHECK (length(version) BETWEEN 1 AND 64),
    target_os TEXT NOT NULL CHECK (length(target_os) BETWEEN 1 AND 32),
    target_arch TEXT NOT NULL CHECK (length(target_arch) BETWEEN 1 AND 32),
    state TEXT NOT NULL CHECK (state IN ('starting', 'ready', 'stopped')),
    started_at TEXT NOT NULL CHECK (length(started_at) BETWEEN 20 AND 64),
    ready_at TEXT CHECK (ready_at IS NULL OR length(ready_at) BETWEEN 20 AND 64),
    stopped_at TEXT CHECK (stopped_at IS NULL OR length(stopped_at) BETWEEN 20 AND 64),
    stop_reason TEXT CHECK (stop_reason IS NULL OR length(stop_reason) BETWEEN 1 AND 64),
    CHECK (
        (state = 'starting' AND ready_at IS NULL AND stopped_at IS NULL AND stop_reason IS NULL)
        OR (state = 'ready' AND ready_at IS NOT NULL AND stopped_at IS NULL AND stop_reason IS NULL)
        OR (state = 'stopped' AND stopped_at IS NOT NULL AND stop_reason IS NOT NULL)
    )
) STRICT;

CREATE INDEX daemon_instances_state_started_idx
    ON daemon_instances (state, started_at);

UPDATE jarvis_storage_metadata
SET schema_version = 2
WHERE singleton = 1;

PRAGMA user_version = 2;