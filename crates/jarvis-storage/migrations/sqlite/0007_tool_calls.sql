-- Tool call records: the bounded invocation, its outcome, and the idempotency ledger.
--
-- Why this table exists is in `docs/architecture/tools-and-connectors.md`, whose execution pipeline
-- ends "Normalize evidence and outcome" then "Persist result and decision receipt", and whose outcome
-- section requires normalizing `requested`, `submitted`, `confirmed`, `failed`, `unknown`, and
-- `cancelled`.
--
-- Design notes that are not obvious from the column list:
--
-- * The `idempotency_key` column IS the idempotency ledger, and `UNIQUE (run_id, idempotency_key)` is
--   the whole mechanism. A separate ledger table would be a second place for the same fact, and the
--   two could disagree. The key is generated once when a call is ADMITTED and persisted, so a pipeline
--   that re-drives a run after a crash finds the existing row by `(run_id, idempotency_key)` rather
--   than creating a second call. A deliberate second call with identical arguments gets a NEW key, so
--   it is a second call -- which is why the key is not the intent hash.
--
-- * `evidence` and `output` are separate columns with very different bounds, because
--   `tools-and-connectors.md` says to preserve "provider IDs and receipts separately from user-facing
--   text". Evidence is a locator (at most 256 characters); output is content (at most 32 KiB) and is
--   the only column holding untrusted provider text.
--
-- * `CHECK (outcome <> 'confirmed' OR evidence IS NOT NULL)` restates the domain's honesty rule so a
--   row from another build is refused too. `P3-001` already makes a `Confirmed` outcome without
--   evidence unconstructible; this makes it unstorable. The restatement is deliberate: the constraint
--   is cheap and a confirmed-without-evidence row is precisely the "success-sounding string into
--   proof" the rule exists to prevent.
--
-- * `outcome` progresses through a state machine that `jarvis-core` owns, and the migration does not
--   try to encode the transition table. A CHECK can express "this row is internally consistent" but
--   not "this row's outcome is a legal successor of the previous one", because that needs the previous
--   row. Exactly the same split as `agent_runs`: CHECK for row properties, Rust for edges.
--
-- * `version` gives an optimistic-concurrency guard, so two writers cannot both record an outcome for
--   one call. The write is a single guarded `UPDATE`, matching `transition_run`'s approach.
--
-- * There is no `arguments` column. Arguments are tool input, which the logging rules exclude from
--   durable records by default, and `intent_hash` already binds the identity of the call. An audit
--   reader needs to know WHICH action was taken, not the payload it carried.
--
-- * `step_id` is nullable with `ON DELETE SET NULL` rather than `CASCADE`: a call outlives the step
--   bookkeeping that referenced it, and a settled run must not lose its record of what it invoked.

CREATE TABLE tool_calls (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    step_id TEXT REFERENCES agent_steps (id) ON DELETE SET NULL,
    tool TEXT NOT NULL CHECK (length(tool) BETWEEN 3 AND 128),
    tool_version TEXT NOT NULL CHECK (length(tool_version) BETWEEN 1 AND 32),
    -- SHA-256 of the canonical intent, lowercase hexadecimal, as in `approvals`.
    intent_hash TEXT NOT NULL CHECK (
        length(intent_hash) = 64 AND intent_hash NOT GLOB '*[^0-9a-f]*'
    ),
    -- 16 random bytes as 32 lowercase hex characters. Unique per run, which is the ledger.
    idempotency_key TEXT NOT NULL CHECK (
        length(idempotency_key) = 32 AND idempotency_key NOT GLOB '*[^0-9a-f]*'
    ),
    -- The authorization receipt, as one JSON document. Kept as a document rather than normalized into
    -- columns because it is a closed value handed to an adapter, and `jarvis-tools` already validates
    -- it: normalizing would give the same facts two homes and two chances to disagree.
    receipt TEXT NOT NULL CHECK (length(CAST(receipt AS BLOB)) BETWEEN 2 AND 4096),
    policy_version TEXT NOT NULL CHECK (length(policy_version) BETWEEN 1 AND 64),
    -- The approval that authorized the call, when one was required. Nullable, because a call policy
    -- allowed directly has none.
    approval_id TEXT REFERENCES approvals (id) ON DELETE SET NULL,
    outcome TEXT NOT NULL CHECK (
        outcome IN (
            'requested', 'authorized', 'submitted', 'confirmed', 'failed', 'unknown', 'cancelled'
        )
    ),
    -- A provider locator, never content. Bounded tightly for that reason.
    evidence TEXT CHECK (evidence IS NULL OR length(evidence) BETWEEN 1 AND 256),
    -- Bounded tool output: the only column holding untrusted provider text.
    output TEXT CHECK (output IS NULL OR length(CAST(output AS BLOB)) <= 32768),
    -- Whether the output was cut. A flag rather than text appended to `output`, so a tool's own
    -- content cannot be mistaken for a truncation marker.
    output_truncated INTEGER NOT NULL DEFAULT 0 CHECK (output_truncated IN (0, 1)),
    -- Why a call failed. A bounded operational fact, never a payload.
    reason TEXT CHECK (reason IS NULL OR length(reason) BETWEEN 1 AND 512),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    -- When the adapter reported. Null until an outcome is reported, and distinct from `updated_at`
    -- because a row can be updated (a version bump) without a new report.
    reported_at TEXT CHECK (reported_at IS NULL OR length(reported_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- A confirmation must carry the evidence that confirmed it.
    CHECK (outcome <> 'confirmed' OR evidence IS NOT NULL),
    -- A failure must carry the reason it failed for.
    CHECK (outcome <> 'failed' OR reason IS NOT NULL),
    -- A truncation flag without output is meaningless, so the pair moves together.
    CHECK (output IS NOT NULL OR output_truncated = 0),
    -- A reported outcome carries a report time; an unreported one does not.
    CHECK (
        (outcome IN ('requested', 'authorized') AND reported_at IS NULL)
        OR (outcome IN ('submitted', 'confirmed', 'failed', 'unknown', 'cancelled')
            AND reported_at IS NOT NULL)
    )
) STRICT;

-- The idempotency ledger: one key per run, so a re-driven pipeline finds the existing call.
CREATE UNIQUE INDEX tool_calls_run_idempotency_idx ON tool_calls (run_id, idempotency_key);

-- A run's calls in the order they were admitted, which the run view needs.
CREATE INDEX tool_calls_run_idx ON tool_calls (run_id, created_at, id);

-- Finding the calls that a re-drive must not repeat: those whose outcome may have had an effect, or
-- which are still in flight. The outcome predicate is an application rule rather than a column, so
-- this index narrows to a run's calls and the filter happens in Rust.
CREATE INDEX tool_calls_outcome_idx ON tool_calls (run_id, outcome);

UPDATE jarvis_storage_metadata
SET schema_version = 7
WHERE singleton = 1;

PRAGMA user_version = 7;
