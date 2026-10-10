-- A project can cap the tokens its runs use in a day (`ADR-0165`). Zero means no cap. Like the run cap, it is enforced where work starts without
-- the owner there, the scheduler; a person's own message in a project is never refused.
ALTER TABLE projects ADD COLUMN daily_token_limit INTEGER NOT NULL DEFAULT 0 CHECK (daily_token_limit BETWEEN 0 AND 2000000000);
UPDATE jarvis_storage_metadata
SET schema_version = 20
WHERE singleton = 1;
PRAGMA user_version = 20;