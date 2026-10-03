-- A pending approval holds the arguments it is waiting on, until it is decided (`P3-028`, ADR-0130).
--
-- `0006` recorded a decision not to store the arguments: "the hash is the binding; storing the arguments would
-- put tool payloads in a durable record ... which the hash already makes unnecessary". The hash does make them
-- unnecessary for **binding**. It cannot make them unnecessary for **deciding**: a person asked to approve
-- `jarvis.web.fetch` has to see the URL, and a person asked to approve a write has to see what is written. With
-- only the tool name and version a human approves a sentence ("a tool wants to run") and the hash binds the
-- decision to an action nobody read.
--
-- It also left the call unresumable by anyone but the party that made it: `POST /calls/{id}/resume` needs the
-- arguments, `tool_calls` stores none, and so a held call could be released only by a client that kept its own
-- copy -- which no human-facing client does.
--
-- # What this does and does not store
--
-- * The arguments are stored **only while the approval is pending**. The decision's guarded `UPDATE` sets the
--   column back to `NULL`, so a decided approval holds no payload. The hash still binds the decision, and the
--   resume route still recomputes it, so a stored payload is not an authority and a wrong one is refused.
-- * Bounded to 8 KiB. A call whose arguments do not fit is held **without** a payload and is therefore not
--   decidable from a client that needs to show it; that fails closed (nobody can approve what they cannot see).
-- * Never returned by anything a model can call, and never logged: the column is read by one route, which is
--   authenticated with the profile credential.
-- * No backfill: existing pending rows have no payload, which is exactly "approved only from a client that
--   holds its own copy", the behaviour they had.
--
-- Recorded as a limit: a pending approval that **expires** keeps its payload until a retention sweep exists.

ALTER TABLE approvals ADD COLUMN arguments_json TEXT CHECK (
    arguments_json IS NULL OR length(arguments_json) <= 8192
);

UPDATE jarvis_storage_metadata
SET schema_version = 13
WHERE singleton = 1;

PRAGMA user_version = 13;
