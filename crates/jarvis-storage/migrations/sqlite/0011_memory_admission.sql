-- Memory admission: who accepted a proposal, and when (`P4-014`).
--
-- `docs/architecture/memory-and-context.md` says the lifecycle ends in "store / supersede / propose
-- review", and a review has two halves: a state and **a decision that names its approver**. The state
-- existed (`proposed` -> `active` through `MemoryRecord::confirm`); the decision did not. There was no
-- column to hold it, so `confirm` could only timestamp the row.
--
-- # Why this is a column pair rather than a row in a decision log
--
-- The same question `ADR-0117` §4 settled for a skill's promotion. "Is this claim trusted, and on whose
-- authority" is asked by the **presentation** path, on every retrieval, for an individual memory -- not
-- by an audit query over a log. A log would make it a join on the hot path and would let the two answers
-- disagree; a column makes "a confirmed claim with nobody behind it" the thing that is unrepresentable.
--
-- The pair follows `skill_revisions.promoted_by_actor_id` / `promoted_at` exactly, including the equality
-- `CHECK`: two columns can otherwise hold half a fact, and half a fact is the shape that reads as complete.
--
-- # Why this is additive, and why the backfill is safe
--
-- `memories` gains columns, so every existing row is untouched and `CURRENT_SCHEMA_VERSION` moves by one.
-- The backfill is **deliberately empty**: the rows that predate admission were confirmed by a route that
-- recorded no approver, so there is no value to migrate -- and inventing one (the local user, or the row's
-- own `created_by_actor_id`) would fabricate an authorization for a decision nobody recorded. A null here
-- means exactly what it should: this claim's acceptance was not attributed.
--
-- # Why the columns carry only their own bounds, and the pair rule lives in the decode
--
-- The rule that matters is a relationship between three columns -- "an `active` row may name an approver,
-- a `proposed` one may not, and the two columns move together" -- and SQLite cannot express it here:
-- `ALTER TABLE ADD COLUMN` takes only a column-def, so a `CHECK` referencing `status` is rejected as soon
-- as it is added, and a table-level `CHECK` would require the twelve-step rebuild that recreates four
-- indexes and two self-referencing foreign keys.
--
-- That rule is therefore enforced on the **read** path, in `MemoryRecord::from_stored`, beside the other
-- status-and-value rule this table already relies on (`deleted` is exactly when the content is empty).
-- The repository's rule is that the schema is the first enforcer and the domain the second; here the
-- domain is the only one, and it refuses a row that arrived from another build, a restored backup, or a
-- hand edit. `docs/data/schema.md` records the same split.
--
-- Recorded as a limit rather than a silent omission: a `proposed` row written directly into the database
-- can carry an approver until something reads it. No code path in this build can produce one -- nothing
-- sets either column except `confirm_by`, which sets the status in the same value.

ALTER TABLE memories ADD COLUMN admitted_by_actor_id TEXT CHECK (
    admitted_by_actor_id IS NULL OR length(admitted_by_actor_id) BETWEEN 1 AND 128
);

ALTER TABLE memories ADD COLUMN admitted_at TEXT CHECK (
    admitted_at IS NULL OR length(admitted_at) BETWEEN 20 AND 64
);

UPDATE jarvis_storage_metadata
SET schema_version = 11
WHERE singleton = 1;

PRAGMA user_version = 11;
