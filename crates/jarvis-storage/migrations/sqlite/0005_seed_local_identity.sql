-- Seeds the single local user and workspace that this profile belongs to.
--
-- Why this migration exists: migration `0003_conversation_state.sql` states in its
-- header that "there is exactly one local user and one local workspace in Phase 2,
-- so those rows are seeded rather than created per client", but no migration ever
-- inserted them. Only the test fixtures did. Every storage test therefore ran
-- against a database whose identity rows it had written itself, and the defect was
-- invisible until a real daemon was asked to create a run: `agent_runs` references
-- `sessions`, `sessions` references `users` and `workspaces`, and a fresh profile
-- had none of them, so `POST /api/v1/runs` could not be satisfied by any client.
--
-- A comment that describes behaviour no code implements is indistinguishable from
-- an unmet obligation, which is the same class of drift `docs/data/schema.md`
-- records for the phantom `config_metadata` and `audit_records` tables. This is the
-- fix for the comment rather than the comment being the fix for the schema.
--
-- The identifiers are FIXED rather than generated. A random ID at migration time
-- would make the same profile's user differ between two installs, so a backup, an
-- export, or a diagnostic would describe a different identity than the one the
-- running daemon uses. Fixed IDs also make the row idempotent in the sense that
-- matters here: migrations run once, and a known value can be referenced by tests
-- and by `docs` without discovery.
--
-- `INSERT OR IGNORE` is deliberate. A profile that already seeded these rows
-- (a developer database created while the fixtures were the only source) must not
-- fail the migration, and it must keep the timestamps it already has rather than
-- having them rewritten by an upgrade.
--
-- Timestamps use `strftime`, matching the RFC 3339 UTC text form the other tables
-- require, so ordering stays lexicographic and the value parses as `UtcTimestamp`.

-- The one local user. `display_name` is intentionally generic: this is the row a
-- single desktop profile owns, not a person's identity, which arrives with
-- authentication in a later phase.
INSERT OR IGNORE INTO users (
    id,
    display_name,
    status,
    created_at,
    updated_at,
    version
) VALUES (
    '0198f000-0000-7000-8000-0000000000b1',
    'Local User',
    'active',
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    1
);

-- The one local workspace. `mode = 'local'` and `data_policy = 'local-only'` are
-- the accurate description of a SQLite-only profile: nothing leaves the machine by
-- default, which is what `docs/product/requirements.md` NFR-PRIV-001 requires. A
-- server deployment creates its own workspace through the server profile rather
-- than inheriting this row.
INSERT OR IGNORE INTO workspaces (
    id,
    name,
    mode,
    data_policy,
    status,
    created_at,
    updated_at,
    version
) VALUES (
    '0198f000-0000-7000-8000-0000000000b2',
    'Local',
    'local',
    'local-only',
    'active',
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    1
);

UPDATE jarvis_storage_metadata
SET schema_version = 5
WHERE singleton = 1;

PRAGMA user_version = 5;
