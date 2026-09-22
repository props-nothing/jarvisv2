-- Durable approval requests: the decision record for an effect that needed a human.
--
-- Why this table exists is in `docs/architecture/security.md`, which describes an approval as
-- "a decision record, not a model message", and in `docs/architecture/events-and-workflows.md`,
-- which states the constraint that decides the design: "Approval is not a permanent bearer token."
--
-- Design notes that are not obvious from the column list:
--
-- * `intent_hash` is the SHA-256 of the canonicalized intent, and it is what the decision binds to.
--   `security.md`: "Editing the action invalidates the approval." The hash is computed by
--   `jarvis-core` from the tool, its version, and the exact arguments, so a decision can be checked
--   against the action it claims to authorize without storing the arguments themselves (which are
--   tool payloads and are excluded from durable records by the logging rules).
--
-- * The one-time decision nonce is stored as `nonce_hash`, NOT as the nonce. The nonce is presented
--   once and compared in constant time by the domain, so a leaked database row must not yield a
--   usable secret. Storing the digest rather than the value is the difference between "the record
--   proves a decision happened" and "the record is a bearer token", which is exactly the failure
--   `events-and-workflows.md` names.
--
-- * `expires_at` is stored as TEXT like every other timestamp, and is **never compared in SQL**.
--   `jarvis_core::UtcTimestamp`'s `Rfc3339` form omits the fraction when it is zero, so `"...00Z"`
--   sorts after `"...00.5Z"` as text and a predicate like `expires_at < ?1` would treat an expired
--   row as unexpired. Expiry is evaluated in Rust from `unix_nanos()`. The index below exists for
--   finding lapsed PENDING rows, and any query using it must filter in Rust or on an integer column.
--
-- * `state` is the state as of the row's last write, and it is deliberately the only place `expired`
--   can appear: a row that was never decided stays `pending` forever, and the *effective* state is
--   `pending` or `expired` depending on the clock. Recording `expired` would require a sweep job for
--   correctness rather than for tidiness, and a row restored from a backup would then be wrong.
--
-- * `CHECK` enforces the decision fields as a unit: a row is either undecided (no decision, no
--   decision fields) or decided (all of them). A partially decided row is unrepresentable, so a
--   reader cannot see a state with a decision and no channel to attribute it to.
--
-- * There is no `payload` or argument column. The preview is a bounded human-readable summary and
--   the hash is the binding; storing the arguments would put tool payloads in a durable record,
--   which the logging rules exclude by default and which the hash already makes unnecessary.

CREATE TABLE approvals (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    -- The actor that requested the action. Not a foreign key: the local profile seeds one user, but
    -- a server deployment's actor may be a client or a runtime identity that is not a `users` row,
    -- and requiring one would make those unrepresentable rather than unauthorized.
    actor_id TEXT NOT NULL CHECK (length(actor_id) BETWEEN 1 AND 128),
    tool TEXT NOT NULL CHECK (length(tool) BETWEEN 3 AND 128),
    tool_version TEXT NOT NULL CHECK (length(tool_version) BETWEEN 1 AND 32),
    -- SHA-256 as lowercase hexadecimal: exactly 64 characters, all of them hex. `NOT GLOB
    -- '*[^0-9a-f]*'` is the idiom for "contains no character outside the class", which is what makes
    -- a non-hex digest unstorable rather than merely unexpected.
    intent_hash TEXT NOT NULL CHECK (
        length(intent_hash) = 64 AND intent_hash NOT GLOB '*[^0-9a-f]*'
    ),
    preview TEXT NOT NULL CHECK (length(preview) BETWEEN 1 AND 512),
    risk_level INTEGER NOT NULL CHECK (risk_level BETWEEN 0 AND 3),
    required_strength TEXT NOT NULL CHECK (
        required_strength IN ('absent', 'channel_evidence', 'credential', 'present')
    ),
    -- The digest of the one-time decision nonce, in the same form as `intent_hash`.
    nonce_hash TEXT NOT NULL CHECK (
        length(nonce_hash) = 64 AND nonce_hash NOT GLOB '*[^0-9a-f]*'
    ),
    state TEXT NOT NULL CHECK (
        state IN ('pending', 'approved', 'denied', 'cancelled', 'expired')
    ),
    decision_channel TEXT CHECK (
        decision_channel IS NULL OR decision_channel IN ('cli', 'desktop', 'api', 'voice')
    ),
    decision_strength TEXT CHECK (
        decision_strength IS NULL
        OR decision_strength IN ('absent', 'channel_evidence', 'credential', 'present')
    ),
    -- The identity that answered. Distinct from `actor_id`, and the domain refuses them being equal.
    decided_by TEXT CHECK (decided_by IS NULL OR length(decided_by) BETWEEN 1 AND 128),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    expires_at TEXT NOT NULL CHECK (length(expires_at) BETWEEN 20 AND 64),
    occurred_at TEXT CHECK (occurred_at IS NULL OR length(occurred_at) BETWEEN 20 AND 64),
    -- A row is undecided or fully decided, never partly. This is what makes "a decision with no
    -- channel" and "a state that disagrees with its decision" unrepresentable rather than a bug a
    -- reader has to defend against.
    CHECK (
        (
            state = 'pending'
            AND decision_channel IS NULL AND decision_strength IS NULL
            AND decided_by IS NULL AND occurred_at IS NULL
        )
        OR (
            state = 'expired'
            AND decision_channel IS NULL AND decision_strength IS NULL
            AND decided_by IS NULL AND occurred_at IS NULL
        )
        OR (
            state IN ('approved', 'denied', 'cancelled')
            AND decision_channel IS NOT NULL AND decision_strength IS NOT NULL
            AND decided_by IS NOT NULL AND occurred_at IS NOT NULL
        )
    )
) STRICT;

-- One approval per intent per run. Two pending approvals for the same action would mean two prompts
-- for one effect, and answering either of them would appear to authorize it. A decided row does not
-- block a NEW intent (a changed argument set hashes differently), so re-asking about a changed
-- action is still possible, which is required: editing the action invalidates the approval.
CREATE UNIQUE INDEX approvals_run_intent_idx ON approvals (run_id, intent_hash);

-- Finding a run's approvals, which the run view and the resume path both need.
CREATE INDEX approvals_run_idx ON approvals (run_id, created_at DESC);

-- Finding approvals that are still awaiting a decision. Deliberately NOT indexed on `expires_at`
-- for the purpose of expiry, because the comparison cannot be done in SQL (see the column note
-- above); this index narrows to the rows a sweep would then check in Rust.
CREATE INDEX approvals_pending_idx ON approvals (state, created_at);

UPDATE jarvis_storage_metadata
SET schema_version = 6
WHERE singleton = 1;

PRAGMA user_version = 6;
