-- The part of a run's conversation that has not been written to the transcript yet (`P9-032`, ADR-0143).
--
-- A run is a loop: the model asks for a tool, the result is fed back, the model carries on. Those in-run turns (the assistant
-- turn that asked, the result of each call) live in memory. When a call is **held** for the owner, the run parks, and the
-- decision may arrive minutes later, after a restart, or never. The first version of the resume path re-assembled the
-- context from the stored transcript and appended the one approved result, which loses every in-run turn: after each
-- approval the model had forgotten what it had already read, written and run, and started the task again. In a real
-- session JARVIS approved the same `npm run build` fourteen times because each approval wiped what it had just learned.
--
-- One row per run, replaced whenever the run parks. `payload_json` is opaque to storage (the executor owns its shape): the
-- in-run messages, the provider's id of the call that is waiting, and the ids of calls in the same turn that did not run.
-- It is read once, when the run resumes, and deleted with the run. Bounded to 1 MiB; the executor drops the oldest whole
-- turns to fit rather than storing a half-turn a provider would reject.

CREATE TABLE run_transcripts (
    run_id TEXT PRIMARY KEY REFERENCES agent_runs (id) ON DELETE CASCADE,
    payload_json TEXT NOT NULL CHECK (length(payload_json) BETWEEN 2 AND 1048576),
    updated_at TEXT NOT NULL
);

UPDATE jarvis_storage_metadata
SET schema_version = 16
WHERE singleton = 1;

PRAGMA user_version = 16;