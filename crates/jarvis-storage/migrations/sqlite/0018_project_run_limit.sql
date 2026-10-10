-- A project can cap how many runs it starts in a day (`ADR-0155`). Zero means no cap. The cap is enforced where work starts without the
-- owner there, the scheduler; a person's own message in a project is never refused.

ALTER TABLE projects ADD COLUMN daily_run_limit INTEGER NOT NULL DEFAULT 0 CHECK (daily_run_limit BETWEEN 0 AND 1000);

UPDATE jarvis_storage_metadata
SET schema_version = 18
WHERE singleton = 1;

PRAGMA user_version = 18;