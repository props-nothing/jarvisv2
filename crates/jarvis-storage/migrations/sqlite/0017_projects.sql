-- Projects: the long-running work JARVIS manages (`ADR-0151`).
--
-- A chat answers a question; a project is something JARVIS keeps working on over days, often unattended, from a schedule. It needs
-- to know what it is for, how the owner wants it done, where its files live, and what has already happened, in every run, without
-- the owner pasting that into each objective. That is what these tables are:
--
-- `projects`       the goal (what done looks like), the standing guidance (how to work: tone, limits, rules of engagement), the working
--                  folder (relative to a granted root) and a status (`active`, `paused`, `done`). The owner writes these.
-- `project_notes`  the journal: short dated entries (`progress`, `decision`, `blocker`, `next`, `result`, `owner`) written by the model
--                  through `jarvis.project.note` or by the owner. The latest are shown to every run of the project, so a run that starts
--                  tomorrow knows what today's run decided. Replaces ad-hoc worklog files the model had to find and re-read.
-- `project_links`  which sessions and schedules belong to a project. A run belongs to a project through its session, so a scheduled
--                  task and every follow-up message in a chat share the project without carrying it in each request.
--
-- Nothing here grants authority: a project changes what a run is told, never what it may do (policy, approvals and the granted folders
-- are unchanged).

CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    name TEXT NOT NULL COLLATE NOCASE CHECK (length(name) BETWEEN 1 AND 80),
    goal TEXT NOT NULL CHECK (length(goal) <= 4000),
    guidance TEXT NOT NULL CHECK (length(guidance) <= 12000),
    folder TEXT NOT NULL CHECK (length(folder) <= 512),
    status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'done')),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    UNIQUE (workspace_id, name)
);

CREATE INDEX projects_workspace_idx ON projects (workspace_id, status);

CREATE TABLE project_notes (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('progress', 'decision', 'blocker', 'next', 'result', 'owner')),
    body TEXT NOT NULL CHECK (length(body) BETWEEN 1 AND 4000),
    run_id TEXT,
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64)
);

CREATE INDEX project_notes_project_idx ON project_notes (project_id, id);

CREATE TABLE project_links (
    kind TEXT NOT NULL CHECK (kind IN ('session', 'schedule')),
    ref_id TEXT NOT NULL CHECK (length(ref_id) = 36),
    project_id TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    PRIMARY KEY (kind, ref_id)
);

CREATE INDEX project_links_project_idx ON project_links (project_id);

UPDATE jarvis_storage_metadata
SET schema_version = 17
WHERE singleton = 1;

PRAGMA user_version = 17;