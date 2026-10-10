-- The people and companies JARVIS is working with (`ADR-0155`): a prospecting project that lives in a CSV file loses its state every time the
-- file is rewritten, and cannot answer "who have we already contacted?" A row is a lead, a customer or anyone else worth remembering,
-- with where it stands. It is local data only: nothing here is sent anywhere, and a model reads it back fenced as data.

CREATE TABLE contacts (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    project_id TEXT REFERENCES projects (id) ON DELETE SET NULL,
    company TEXT NOT NULL CHECK (length(company) BETWEEN 1 AND 200),
    person TEXT NOT NULL DEFAULT '' CHECK (length(person) <= 200),
    role TEXT NOT NULL DEFAULT '' CHECK (length(role) <= 200),
    email TEXT NOT NULL DEFAULT '' CHECK (length(email) <= 254),
    phone TEXT NOT NULL DEFAULT '' CHECK (length(phone) <= 40),
    status TEXT NOT NULL DEFAULT 'new' CHECK (
        status IN ('new', 'contacted', 'replied', 'meeting', 'won', 'lost', 'do_not_contact')
    ),
    notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 2000),
    source TEXT NOT NULL DEFAULT '' CHECK (length(source) <= 300),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64)
) STRICT;

-- One row per address, and one per company and person when there is no address, so saving the same lead twice updates it.
CREATE UNIQUE INDEX contacts_email_idx ON contacts (workspace_id, lower(email)) WHERE email <> '';
CREATE UNIQUE INDEX contacts_person_idx ON contacts (workspace_id, lower(company), lower(person)) WHERE email = '';
CREATE INDEX contacts_status_idx ON contacts (workspace_id, status);

UPDATE jarvis_storage_metadata
SET schema_version = 19
WHERE singleton = 1;

PRAGMA user_version = 19;
