PRAGMA application_id = 1245794902;

CREATE TABLE jarvis_storage_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    created_at_unix_ms INTEGER NOT NULL CHECK (created_at_unix_ms >= 0)
) STRICT;

INSERT INTO jarvis_storage_metadata (
    singleton,
    schema_version,
    created_at_unix_ms
) VALUES (
    1,
    1,
    unixepoch() * 1000
);

PRAGMA user_version = 1;