-- Skills: a procedure that names already-granted tools and never authority (`ADR-0117`).
--
-- `ADR-0117` fixes a trust boundary, and this schema is where three of its rules become properties of a
-- row rather than of a code path:
--
-- 1. **§3 — a self-authored procedure is `Derived` at best.** Restated as the same source-kind/trust
--    equality the memories table carries, so a model-authored skill cannot claim to be the user instruction.
-- 2. **§4 — promotion is an approval that names its approver.** Enforced as a conditional: a
--    model-authored **active** row must record who promoted it. The domain cannot enforce this on a
--    *decode* (a promoted revision legitimately decodes as active), so the schema is the only layer that can.
-- 3. **§5 — replacement is declared.** Both supersession directions are columns, so "which procedure ran"
--    is answerable from the record rather than reconstructed by comparing prose.
--
-- # Why `procedural` memories and skills are separate tables
--
-- `FR-MEM-001` lists `procedural` among the memory types, so a skill *could* have been `memories` rows with
-- a JSON body. It is not, for the reason `ADR-0117`'s consequences state: a skill is **not** a claim. A
-- claim has confidence, decay, retrieval ranking, and a search key; a procedure has versioned steps, a
-- promotion, and a supersession chain. Putting a procedure in `memories` would make every retrieval query
-- carry a type predicate to skip the rows that are not claims, and would let a skill silently acquire a
-- claim's semantics — confidence and decay applied to a procedure is meaningless, and a `search_key`
-- collapsed onto a procedure would deduplicate two different revision bodies.
--
-- So `procedural` remains a valid `memory_type` (the vocabulary is `FR-MEM-001`'s) and a **skill is a
-- different record shape**, recorded here as a departure rather than left for a reader to infer.

CREATE TABLE skill_revisions (
    -- The revision's own identity, which is what is stored and audited. A revision needs an identity
    -- separate from the skill's, because "which revision of this procedure ran" must be answerable and a
    -- version *string* is content the author chose rather than a value the platform issued.
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    -- The skill this revision belongs to, stable across revisions. **Not** a foreign key to a skills table,
    -- because no such table is needed: the skill is the identity its revisions share, and every fact about
    -- it (its description, its steps) belongs to a specific revision. A parent table would hold a name and
    -- nothing else, and a row whose only content is an identifier the children already carry is a join for
    -- no fact — the shape this repository removes.
    skill_id TEXT NOT NULL CHECK (length(skill_id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    -- The author's version string. Content, not identity, and bounded like a tool version so a step's
    -- `tool_version` and this cannot disagree about what a version looks like.
    version TEXT NOT NULL CHECK (length(version) BETWEEN 1 AND 64),
    -- The procedure's prose, which reaches a model and is therefore fenced by the caller rather than here.
    -- Bounded because an unbounded description is a single record that can consume a whole context budget.
    description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 8192),
    -- The ordered steps, as a JSON array of `{position, tool, tool_version, instruction}`.
    --
    -- JSON rather than a `skill_steps` table, and this is a departure from the rule the memories module
    -- states ("JSON stores versioned provider payload fragments or flexible metadata, not core relationships
    -- that need constraints"). The rule's *reason* is that a core relationship gets columns so a lookup is an
    -- index seek and a reader does not depend on a document shape. A step list fails both halves for a
    -- **procedure**: nothing queries "which skills name tool X" on a hot path (and `P4-012`'s selection is by
    -- relevance to a task, not by tool), and a step is *only* meaningful as part of its revision — a step row
    -- detached from its revision names an action with no place in a sequence, which is not a value anything
    -- should be able to hold.
    --
    -- The shape `jarvis_core::RunEventPayload` uses for the same reason, and the constraint below is what
    -- keeps the rule's *intent*: the document must be a bounded array, so it cannot be an unbounded blob.
    steps TEXT NOT NULL CHECK (
        json_valid(steps)
        AND json_type(steps) = 'array'
        AND json_array_length(steps) BETWEEN 1 AND 64
    ),
    -- Provenance. All three `NOT NULL`: a procedure with no source is not a skill, which is
    -- `ADR-0117`'s central rule and therefore the schema's. The same three columns the memories table
    -- carries, deliberately, so a reader of either table reads one vocabulary.
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'user_statement', 'user_correction', 'provider_record',
            'tool_observation', 'document', 'model_inference', 'external_content'
        )
    ),
    source_locator TEXT NOT NULL CHECK (length(source_locator) BETWEEN 1 AND 512),
    source_trust TEXT NOT NULL CHECK (
        source_trust IN ('untrusted', 'derived', 'authoritative')
    ),
    source_excerpt_hash TEXT CHECK (
        source_excerpt_hash IS NULL
        OR (length(source_excerpt_hash) = 64 AND source_excerpt_hash NOT GLOB '*[^0-9a-f]*')
    ),
    -- `proposed` is where an agent-authored skill starts and is not decoration: `ADR-0117` §4 makes
    -- promotion an approval, so a proposal becomes usable only through a durable attributable decision.
    state TEXT NOT NULL CHECK (state IN ('proposed', 'active', 'archived')),
    -- The external fields this revision's source document carried and this platform refused, as a bounded
    -- JSON array of `{name, reason}`.
    --
    -- Stored rather than discarded because `ADR-0117` §7 requires the drop to be **recorded**: a field
    -- accepted-then-ignored is worse than one never accepted, since a reader of the stored skill cannot
    -- otherwise tell the format's intent from this platform's behaviour.
    dropped_fields TEXT NOT NULL CHECK (
        json_valid(dropped_fields)
        AND json_type(dropped_fields) = 'array'
        AND json_array_length(dropped_fields) <= 64
    ),
    -- Who promoted this revision, when someone did. Absent for a user-authored revision, which is active
    -- from the outset and needed no promotion.
    promoted_by_actor_id TEXT CHECK (
        promoted_by_actor_id IS NULL OR length(promoted_by_actor_id) BETWEEN 1 AND 128
    ),
    promoted_at TEXT CHECK (promoted_at IS NULL OR length(promoted_at) BETWEEN 20 AND 64),
    -- Both supersession directions, for the reason the memories table gives: "is this still current" is on
    -- the read path, and following one direction would make it a scan of every later revision.
    supersedes_revision_id TEXT REFERENCES skill_revisions (id) ON DELETE SET NULL,
    superseded_by_revision_id TEXT REFERENCES skill_revisions (id) ON DELETE SET NULL,
    run_id TEXT REFERENCES agent_runs (id) ON DELETE SET NULL,
    created_by_actor_id TEXT NOT NULL CHECK (length(created_by_actor_id) BETWEEN 1 AND 128),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version_counter INTEGER NOT NULL CHECK (version_counter >= 1),
    -- **The injection boundary, restated from the memories table.** External content cannot claim to be
    -- authoritative, and a model inference is derived. The domain refuses this at construction; the schema
    -- refuses it at write, so a row that arrived some other way is still refused (`ADR-0117` §3).
    CHECK (
        (source_kind IN ('user_statement', 'user_correction', 'provider_record')
            AND source_trust = 'authoritative')
        OR (source_kind IN ('tool_observation', 'document', 'model_inference')
            AND source_trust = 'derived')
        OR (source_kind = 'external_content' AND source_trust = 'untrusted')
    ),
    -- **⭐ The promotion rule, in SQL, and this is the constraint only the schema can state.**
    --
    -- The domain refuses a *construction* of a model-authored active revision, because promotion is an
    -- approval (`ADR-0117` §4). But a **decode** must be able to read back an active model-authored revision
    -- — that is exactly what a promotion produces — so the domain's check cannot cover the stored row, and a
    -- row that became active some other way would pass it.
    --
    -- This is that gap closed: a model-produced revision that is active **must record who promoted it**.
    -- A user-authored revision is exempt because it is active from the outset and no decision made it so.
    CHECK (
        state <> 'active'
        OR source_kind <> 'model_inference'
        OR promoted_by_actor_id IS NOT NULL
    ),
    -- A promotion's attribution and its instant travel together: half of a decision's provenance is not a
    -- decision. Without this, a row could name an approver and no time, and the audit question "when was this
    -- approved" would have no answer.
    CHECK ((promoted_by_actor_id IS NULL) = (promoted_at IS NULL)),
    -- Neither supersession direction may be a self-reference: the chain must be walkable.
    CHECK (supersedes_revision_id IS NULL OR supersedes_revision_id <> id),
    CHECK (superseded_by_revision_id IS NULL OR superseded_by_revision_id <> id),
    -- A revision is not its own skill. Distinct from the check above: that one is about the chain, this one
    -- about identity, and a decoded row could satisfy either without the other.
    CHECK (skill_id <> id)
) STRICT;

-- **One revision per (skill, author version) per workspace.**
--
-- Two rows claiming the same version of one procedure is a contradiction to surface rather than a second row
-- to store — the rule the identity-alias index applies to a verified alias. The author's version string is
-- content, so this is *not* an identity constraint: two different revisions may legitimately carry "1" in
-- different skills, and the same skill may later declare "2".
CREATE UNIQUE INDEX skill_revisions_version_idx
    ON skill_revisions (workspace_id, skill_id, version);

-- "Which revisions of this skill exist", newest first, which is what an inspection read asks.
CREATE INDEX skill_revisions_skill_idx
    ON skill_revisions (workspace_id, skill_id, created_at DESC);

-- The eligibility filter `P4-012`'s selection runs: a workspace's usable revisions.
--
-- Partial on `state = 'active'`, because a proposal or an archived revision is never offered for use, and the
-- predicate being in the index means the filter is a seek rather than a scan over rows that are candidates
-- only for review.
CREATE INDEX skill_revisions_usable_idx
    ON skill_revisions (workspace_id, created_at DESC)
    WHERE state = 'active';

-- "What replaced this" and "what did this replace", both directions, for the reason the memories table gives.
CREATE INDEX skill_revisions_supersedes_idx
    ON skill_revisions (supersedes_revision_id)
    WHERE supersedes_revision_id IS NOT NULL;

UPDATE jarvis_storage_metadata
SET schema_version = 10
WHERE singleton = 1;

PRAGMA user_version = 10;
