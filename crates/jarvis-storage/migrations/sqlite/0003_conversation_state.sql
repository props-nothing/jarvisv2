-- Phase 2 conversation state: identity, sessions, messages, runs, steps, and model
-- calls.
--
-- Design notes that are not obvious from the column list:
--
-- * Timestamps are RFC 3339 UTC text, matching `daemon_instances`, so ordering is
--   lexicographic and every row round-trips through `UtcTimestamp` without a
--   lossy numeric conversion.
-- * `agents_runs` rows carry `version` for optimistic concurrency. A state change
--   must supply the version it read, so two writers cannot both advance a run.
-- * `agent_runs.state` is constrained to the states in the run state machine.
--   `terminal_outcome` and `error_code` are populated only in a terminal state,
--   which makes an "already failed but still running" row unrepresentable.
-- * The run state machine's legal transitions are enforced in Rust, not by a
--   trigger. A trigger would need the previous row to decide, and it would be
--   invisible to the application. The transition table belongs to P2-005.
-- * `model_calls` records usage inline. `docs/data/schema.md` defines no separate
--   usage table: `agent_runs` carries a usage/cost summary and `model_calls`
--   carries the per-call dimensions, and a duplicate total would be able to
--   disagree with its parts.
-- * Secret values never appear here. `provider_request_id` is an opaque trace
--   reference, and error text is a JARVIS-owned safe message, never the provider's
--   own message, which can echo the prompt.
-- * There is exactly one local user and one local workspace in Phase 2. Those rows
--   are seeded by migration `0005_seed_local_identity.sql` rather than created per
--   client. That migration exists because this file originally CLAIMED the seeding
--   happened while no migration performed it, so a fresh profile had no identity
--   for `sessions` to reference and no client could create a run.

CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 128),
    locale TEXT CHECK (locale IS NULL OR length(locale) BETWEEN 2 AND 35),
    timezone TEXT CHECK (timezone IS NULL OR length(timezone) BETWEEN 1 AND 64),
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1)
) STRICT;

CREATE TABLE workspaces (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 128),
    mode TEXT NOT NULL CHECK (mode IN ('local', 'server')),
    data_policy TEXT NOT NULL CHECK (data_policy IN ('standard', 'local-only')),
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1)
) STRICT;

CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE RESTRICT,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    channel TEXT NOT NULL CHECK (channel IN ('cli', 'desktop', 'api', 'voice')),
    title TEXT CHECK (title IS NULL OR length(title) BETWEEN 1 AND 256),
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    model_id TEXT CHECK (model_id IS NULL OR length(model_id) BETWEEN 1 AND 128),
    provider_id TEXT CHECK (provider_id IS NULL OR length(provider_id) BETWEEN 1 AND 128),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    archived_at TEXT CHECK (archived_at IS NULL OR length(archived_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1),
    CHECK (
        (status = 'active' AND archived_at IS NULL)
        OR (status = 'archived' AND archived_at IS NOT NULL)
    )
) STRICT;

CREATE INDEX sessions_workspace_updated_idx ON sessions (workspace_id, updated_at DESC);

CREATE TABLE agent_runs (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    session_id TEXT NOT NULL REFERENCES sessions (id) ON DELETE RESTRICT,
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE RESTRICT,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    objective TEXT NOT NULL CHECK (length(objective) BETWEEN 1 AND 4096),
    state TEXT NOT NULL CHECK (
        state IN (
            'received', 'context_building', 'planning', 'executing', 'observing',
            'awaiting_approval', 'responding', 'completed', 'cancelled', 'failed'
        )
    ),
    runtime_id TEXT CHECK (runtime_id IS NULL OR length(runtime_id) BETWEEN 1 AND 128),
    provider_id TEXT CHECK (provider_id IS NULL OR length(provider_id) BETWEEN 1 AND 128),
    model_id TEXT CHECK (model_id IS NULL OR length(model_id) BETWEEN 1 AND 128),
    cancellation_requested_at TEXT CHECK (cancellation_requested_at IS NULL OR length(cancellation_requested_at) BETWEEN 20 AND 64),
    terminal_outcome TEXT CHECK (terminal_outcome IS NULL OR terminal_outcome IN ('succeeded', 'cancelled', 'failed')),
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 64),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
    started_at TEXT NOT NULL CHECK (length(started_at) BETWEEN 20 AND 64),
    completed_at TEXT CHECK (completed_at IS NULL OR length(completed_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- A run is terminal exactly when it has an outcome, an end time, and (for
    -- failure) an error code. Making the two groups inseparable means "failed but
    -- still running" or "completed with no outcome" cannot be stored.
    CHECK (
        (state = 'completed' AND terminal_outcome = 'succeeded' AND completed_at IS NOT NULL AND error_code IS NULL)
        OR (state = 'cancelled' AND terminal_outcome = 'cancelled' AND completed_at IS NOT NULL AND error_code IS NULL)
        OR (state = 'failed' AND terminal_outcome = 'failed' AND completed_at IS NOT NULL AND error_code IS NOT NULL)
        OR (state NOT IN ('completed', 'cancelled', 'failed') AND terminal_outcome IS NULL AND completed_at IS NULL AND error_code IS NULL)
    )
) STRICT;

CREATE INDEX agent_runs_session_started_idx ON agent_runs (session_id, started_at DESC);
CREATE INDEX agent_runs_active_idx ON agent_runs (state, updated_at)
    WHERE state NOT IN ('completed', 'cancelled', 'failed');

-- Declared after `agent_runs` on purpose: `run_id` references it, and a forward
-- reference would make SQLite resolve the parent lazily. Because a null `run_id`
-- skips foreign-key checking entirely, a broken forward reference would pass every
-- test that stored a message without a run and fail only for the first writer that
-- set one. Parent-before-child removes that failure mode instead of testing for it.
CREATE TABLE messages (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    session_id TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    author_kind TEXT NOT NULL CHECK (author_kind IN ('user', 'assistant', 'system', 'tool')),
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content TEXT NOT NULL,
    -- Derived from `content` rather than trusted from the writer. A stored size that
    -- could disagree with the text it describes is the same class of defect as a
    -- duplicated usage total: two values that can drift with no way to tell which is
    -- right. The CHECK makes disagreement unrepresentable.
    --
    -- `length()` measures CHARACTERS for a TEXT value but BYTES for a BLOB, so the
    -- cast is required. Without it this constraint would enforce a character count
    -- while `jarvis_models` budgets bytes, and every message containing non-ASCII
    -- text would be rejected by its own size check. Casting also avoids TEXT's
    -- truncation at an embedded NUL.
    content_bytes INTEGER NOT NULL CHECK (content_bytes = length(CAST(content AS BLOB))),
    sensitivity TEXT NOT NULL CHECK (sensitivity IN ('public', 'internal', 'confidential', 'restricted')),
    source TEXT NOT NULL CHECK (source IN ('user', 'model', 'tool', 'system')),
    -- Nullable because a user message is stored before the run it triggers exists.
    -- When set it must resolve. `SET NULL` rather than `CASCADE`, because a message
    -- belongs to its session and outlives the run that happened to produce it.
    run_id TEXT REFERENCES agent_runs (id) ON DELETE SET NULL,
    provider_message_id TEXT CHECK (provider_message_id IS NULL OR length(provider_message_id) BETWEEN 1 AND 128),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    UNIQUE (session_id, sequence)
) STRICT;

CREATE INDEX messages_session_sequence_idx ON messages (session_id, sequence);

CREATE INDEX messages_run_idx ON messages (run_id) WHERE run_id IS NOT NULL;

CREATE TABLE agent_steps (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    run_id TEXT NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    step_key TEXT NOT NULL CHECK (length(step_key) BETWEEN 1 AND 128),
    attempt_group INTEGER NOT NULL CHECK (attempt_group >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('context', 'plan', 'model_call', 'tool_call', 'observation', 'response')),
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    -- A bounded operational summary only. Hidden chain-of-thought is never stored.
    summary TEXT CHECK (summary IS NULL OR length(summary) BETWEEN 1 AND 1024),
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 64),
    started_at TEXT NOT NULL CHECK (length(started_at) BETWEEN 20 AND 64),
    ended_at TEXT CHECK (ended_at IS NULL OR length(ended_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1),
    UNIQUE (run_id, sequence),
    CHECK (
        (state IN ('pending', 'running') AND ended_at IS NULL)
        OR (state IN ('succeeded', 'failed', 'cancelled') AND ended_at IS NOT NULL)
    )
) STRICT;

CREATE INDEX agent_steps_run_sequence_idx ON agent_steps (run_id, sequence);

CREATE TABLE model_calls (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    run_id TEXT NOT NULL REFERENCES agent_runs (id) ON DELETE CASCADE,
    step_id TEXT REFERENCES agent_steps (id) ON DELETE SET NULL,
    provider_id TEXT NOT NULL CHECK (length(provider_id) BETWEEN 1 AND 128),
    model_id TEXT NOT NULL CHECK (length(model_id) BETWEEN 1 AND 128),
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    status TEXT NOT NULL CHECK (status IN ('succeeded', 'failed', 'cancelled')),
    streaming INTEGER NOT NULL CHECK (streaming IN (0, 1)),
    prompt_hash TEXT NOT NULL CHECK (length(prompt_hash) BETWEEN 16 AND 128),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
    latency_ms INTEGER CHECK (latency_ms IS NULL OR latency_ms >= 0),
    time_to_first_token_ms INTEGER CHECK (time_to_first_token_ms IS NULL OR time_to_first_token_ms >= 0),
    finish_reason TEXT CHECK (finish_reason IS NULL OR length(finish_reason) BETWEEN 1 AND 32),
    provider_request_id TEXT CHECK (provider_request_id IS NULL OR length(provider_request_id) BETWEEN 1 AND 128),
    -- A JARVIS-owned safe message. The provider's own text is deliberately not
    -- stored because it can echo prompt content or interpolate a credential.
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 64),
    error_message TEXT CHECK (error_message IS NULL OR length(error_message) BETWEEN 1 AND 512),
    started_at TEXT NOT NULL CHECK (length(started_at) BETWEEN 20 AND 64),
    ended_at TEXT CHECK (ended_at IS NULL OR length(ended_at) BETWEEN 20 AND 64),
    CHECK (
        (status = 'succeeded' AND ended_at IS NOT NULL AND error_code IS NULL)
        OR (status IN ('failed', 'cancelled') AND ended_at IS NOT NULL AND error_code IS NOT NULL)
    )
) STRICT;

CREATE INDEX model_calls_run_attempt_idx ON model_calls (run_id, started_at, attempt);

UPDATE jarvis_storage_metadata
SET schema_version = 3
WHERE singleton = 1;

PRAGMA user_version = 3;
