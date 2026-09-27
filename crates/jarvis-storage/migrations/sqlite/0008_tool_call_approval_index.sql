-- Makes a held call findable by the approval it is waiting on.
--
-- Why this exists is easiest to state as the gap it closes. `P3-012a` made a held call write a durable
-- approval; `P3-012b` made that approval decidable over HTTP. Both left the two rows **disconnected**:
-- `tool_calls.approval_id` — the column `0007` already declared, with its foreign key — was never
-- written by anything. So a decision moved the approval to `approved` and the call it authorized stayed
-- `requested` forever, with no way to find one row from the other.
--
-- **The schema needed no change; only its writer was missing.** `approval_id` was already nullable, had
-- the right type, and already carried `REFERENCES approvals (id) ON DELETE SET NULL`. The one thing the
-- schema could not do is answer the lookup efficiently, which is all this migration adds.
--
-- Design notes that are not obvious from the single statement below:
--
-- * The index is **partial** (`WHERE approval_id IS NOT NULL`). Only a held call has a link; every call
--   that ran without an approval has a NULL there. A full index would grow with ordinary activity to
--   answer a question only ever asked about held calls, and held calls are the rare case by design.
--
-- * The column is left exactly as `0007` declared it. Re-creating the table to "tidy" it would have
--   meant re-stating every CHECK by hand, and two of those bounds are expressed in **bytes**
--   (`length(CAST(receipt AS BLOB))`, `length(CAST(output AS BLOB))`) precisely so a multi-byte payload
--   cannot smuggle past a character count. Re-typing them is how a byte bound silently becomes a
--   character bound, widening what is storable in the column holding untrusted provider text. Adding an
--   index cannot do that, so the migration does not touch the table.
--
-- * There is **no backfill**. An existing profile's held calls keep `approval_id IS NULL` and stay that
--   way. A backfill would have to guess the link by matching `run_id` and `intent_hash` against
--   `approvals`, and a guess that is merely *usually* right would connect a call to a decision that was
--   never about it. An unlinked call is visible — it stays `requested` and never resumes — whereas a
--   wrongly linked call would run under an authority it never obtained. The safe direction for a missing
--   link is "does not run".
--
-- * `intent_hash` is **not** duplicated here. `tool_calls.intent_hash` already holds the same canonical
--   digest `approvals.intent_hash` holds, and it is what the resume path compares a decision against.
--   A second copy under a new name would be two values that can disagree about one action.

-- The lookup the resume path needs: the call waiting on a given approval. Partial, because the rows with
-- a NULL link are the ones this index would only add weight to.
CREATE INDEX tool_calls_approval_idx ON tool_calls (approval_id) WHERE approval_id IS NOT NULL;

-- Note on what is deliberately NOT expressed here: "a held call has an `approval_id`" is not a CHECK.
-- The state is not a property of a row but of a moment — `requested` is also the state of every call
-- during the window between `admit_tool_call` and the hold write, so a constraint would forbid the
-- correct intermediate row. The invariant is enforced where it can be: the resume path refuses to run an
-- unlinked call, and the write that would set the link is guarded on the call still being `requested`.

UPDATE jarvis_storage_metadata
SET schema_version = 8
WHERE singleton = 1;

PRAGMA user_version = 8;
