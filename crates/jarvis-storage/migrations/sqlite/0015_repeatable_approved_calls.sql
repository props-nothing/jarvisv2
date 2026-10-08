-- A command may be run again once it was approved and has finished (`P9-031`, ADR-0143).
--
-- `0006` made `(run_id, intent_hash)` unique for every approval: "one approval per action per run", so a model that was
-- told no could not ask again, and two identical requests could not both be pending. That is right for **pending** and
-- **denied**, and wrong for **approved**: building a project is a loop (run the build, fix what it reported, run it again),
-- and the second `npm run build` is a new action in a changed world, not a replay of the first. With the index as it was, the
-- second run was refused ("this exact call was already asked about"), and a model that cannot re-run its build cannot verify
-- its own fix. In one live session JARVIS made fourteen different spellings of the same command to get around it.
--
-- The rule stays where it protects someone:
--
-- * **pending**: a second identical request while the first waits is still refused (the owner is not asked twice);
-- * **denied**: the same request after a "no" is still refused (a model does not wear down an answer by asking again).
--
-- An **approved**, cancelled or expired approval no longer blocks a new one, which is asked about afresh: the owner is still
-- asked, every time, so no authority is carried over. The unique index becomes partial.

DROP INDEX approvals_run_intent_idx;

CREATE UNIQUE INDEX approvals_run_intent_idx
    ON approvals (run_id, intent_hash)
    WHERE state IN ('pending', 'denied');

UPDATE jarvis_storage_metadata
SET schema_version = 15
WHERE singleton = 1;

PRAGMA user_version = 15;