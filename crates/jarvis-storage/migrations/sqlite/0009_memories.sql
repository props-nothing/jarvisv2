-- Canonical memory: memories, entities, aliases, relations, and deletion tombstones.
--
-- `docs/architecture/memory-and-context.md` defines the vocabulary, `crates/jarvis-core/src/memory.rs`
-- (P4-001) is its typed form, and this migration is the durable shape. The module doc argues each field;
-- what follows is why the *schema* is shaped the way it is, which is a different set of decisions.
--
-- # The rule this migration exists to make unstorable
--
-- The architecture's central claim is that "a memory is a sourced claim with lifecycle metadata, not an
-- unqualified string". So `source_kind`, `source_locator`, and `source_trust` are `NOT NULL` — a row with no
-- source cannot be written — and the trust/kind pair is checked rather than merely stored. See the
-- `CHECK` on the two columns below: it restates `MemorySourceKind::permitted_trust` in SQL, which means a
-- **hand-edited or restored row** cannot claim authoritative external content either. The domain refuses it
-- at construction and the schema refuses it at write, so the injection boundary has two independent
-- enforcers rather than one that a bypass could evade.
--
-- # Deletion removes derived text, and a tombstone blocks resurrection
--
-- These two requirements pull against each other and the resolution is the interesting part:
--
-- * "Deleting it removes text and derived indexes" — so deletion must clear `content`, `structured_claim`,
--   and `search_key`, which is derived from the content.
-- * A deleted memory must not come back. Re-ingesting the same observation would otherwise insert a fresh
--   row and resurrect what the user deleted.
--
-- A search key cannot serve as the resurrection block, because it *is* derived text: `search_key` for
-- "prefers dark roast coffee" holds the words `coffee dark prefers roast`, so keeping it after deletion
-- would retain content the user asked to remove. So the block is `memory_tombstones.search_key_hash`, a
-- **SHA-256 of the key** — enough to recognise the same claim on a re-ingest, and not enough to read it back.
-- The acceptance invariant is satisfied and resurrection is still refused.
--
-- `memory_tombstones.memory_id` is deliberately **not** a foreign key: a full user-deletion removes the
-- memory row itself (`P4-008`), and the tombstone must outlive it — otherwise deleting a memory would delete
-- the evidence that it was deleted, which is precisely backwards.
--
-- # What the `CHECK`s enforce, and the one rule they cannot
--
-- Restated from the domain so a row written by another build, restored from a backup, or hand-edited is
-- still refused:
--
-- * a source's trust matches its kind (the injection boundary, above);
-- * a `model_inference` is `unverified` — the model cannot assert a fact;
-- * a `provider_record` does not back a `preference` — trust does not transfer between claim kinds;
-- * a memory does not supersede itself, in either direction;
-- * `status = 'deleted'` is exactly when content is empty, so a deleted row cannot retain text and a live row
--   cannot be blank;
-- * an entity is not its own relation subject and object, and a merged entity names where it went.
--
-- The rule that is **not** here is "a relationship memory starts as a proposal". `P4-001` enforces it at
-- construction, and a table `CHECK` cannot: `CHECK`s apply to every write, so `memory_type <> 'relationship'
-- OR status <> 'active'` would forbid *confirming* a relationship memory later, which the slice requires. A
-- constraint that cannot distinguish insert from update cannot express an initial-state rule, and pretending
-- otherwise would break the confirm path.
--
-- # No embedding columns
--
-- `memory_embeddings` is `P4-005`, and the canonical record's embedding fields are absent here on purpose: a
-- memory must be fully usable without them, because "embeddings are one signal" and not a requirement. An
-- unused nullable column set would be a schema that implies retrieval needs an index this build cannot build.

CREATE TABLE entities (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'person', 'organization', 'project', 'document', 'account',
            'device', 'location', 'event', 'task', 'conversation'
        )
    ),
    label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 256),
    -- Normalized attributes as one bounded JSON document, like `receipt` on `tool_calls`: a closed value
    -- whose shape the domain validates, and normalizing it into columns would give the same facts two homes.
    attributes TEXT CHECK (
        attributes IS NULL OR length(CAST(attributes AS BLOB)) BETWEEN 2 AND 8192
    ),
    confidence TEXT NOT NULL CHECK (
        confidence IN ('unverified', 'uncertain', 'likely', 'confirmed')
    ),
    status TEXT NOT NULL CHECK (status IN ('active', 'merged', 'archived', 'deleted')),
    -- Where this entity went when it was merged. A merge is auditable and reversible, so the losing row is
    -- retained and points at the winner rather than being deleted.
    merged_into TEXT REFERENCES entities (id) ON DELETE SET NULL,
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- A merged entity must say where it went, or the merge is unreadable.
    CHECK (status <> 'merged' OR merged_into IS NOT NULL),
    -- An entity cannot be its own merge target: that would make the chain unresolvable.
    CHECK (merged_into IS NULL OR merged_into <> id)
) STRICT;

-- Finding entities in a workspace, which is what a label search and an entity list both do.
CREATE INDEX entities_workspace_kind_idx ON entities (workspace_id, kind, label);

CREATE TABLE entity_aliases (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    entity_id TEXT NOT NULL REFERENCES entities (id) ON DELETE CASCADE,
    -- The workspace is denormalized from the entity so the uniqueness rule below can be scoped without a
    -- join. An alias's scope IS the workspace: the same email in two workspaces is two identities.
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    alias_kind TEXT NOT NULL CHECK (
        alias_kind IN ('name', 'email', 'handle', 'phone', 'provider_id', 'url', 'other')
    ),
    alias_value TEXT NOT NULL CHECK (length(alias_value) BETWEEN 1 AND 256),
    -- The case- and whitespace-folded form the uniqueness rule and a lookup both compare. Stored rather
    -- than derived so the comparison cannot depend on the reader applying the same folding.
    normalized TEXT NOT NULL CHECK (length(normalized) BETWEEN 1 AND 256),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'user_statement', 'user_correction', 'provider_record',
            'tool_observation', 'document', 'model_inference', 'external_content'
        )
    ),
    -- How the alias was established. `probabilistic` is a candidate rather than a fact, which is what the
    -- partial unique index below keys on.
    verification TEXT NOT NULL CHECK (
        verification IN ('confirmed', 'provider_id', 'exact_identifier', 'probabilistic')
    ),
    confidence TEXT NOT NULL CHECK (
        confidence IN ('unverified', 'uncertain', 'likely', 'confirmed')
    ),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    -- A verified alias cannot come from an unverified source: "the model guessed an email and we treat it as
    -- established" is the shape this refuses.
    CHECK (verification <> 'confirmed' OR source_kind IN ('user_statement', 'user_correction')),
    CHECK (verification <> 'probabilistic' OR confidence <> 'confirmed')
) STRICT;

-- **One entity per verified alias per workspace.**
--
-- Partial, and the partiality is the architecture's rule rather than a tuning choice: "Ambiguous aliases
-- remain separate candidates." A *verified* alias is an identity claim, so two entities holding one is a
-- contradiction to surface rather than a second row to store. A *probabilistic* match is a candidate, so two
-- entities may both be guessed from the same name — which is exactly what "remain separate candidates" means,
-- and a full unique index would refuse the second guess and silently pick a winner.
CREATE UNIQUE INDEX entity_aliases_verified_idx
    ON entity_aliases (workspace_id, alias_kind, normalized)
    WHERE verification <> 'probabilistic';

-- Finding an entity's aliases, which resolution does on the way in.
CREATE INDEX entity_aliases_entity_idx ON entity_aliases (entity_id);

CREATE TABLE entity_relations (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    subject_id TEXT NOT NULL REFERENCES entities (id) ON DELETE CASCADE,
    predicate TEXT NOT NULL CHECK (length(predicate) BETWEEN 1 AND 256),
    object_id TEXT NOT NULL REFERENCES entities (id) ON DELETE CASCADE,
    -- A relation is a claim, so it carries provenance and lifecycle exactly as a memory does.
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
    confidence TEXT NOT NULL CHECK (
        confidence IN ('unverified', 'uncertain', 'likely', 'confirmed')
    ),
    sensitivity TEXT NOT NULL CHECK (
        sensitivity IN ('public', 'internal', 'confidential', 'restricted')
    ),
    status TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'archived', 'deleted')),
    valid_from TEXT NOT NULL CHECK (length(valid_from) BETWEEN 20 AND 64),
    valid_until TEXT CHECK (valid_until IS NULL OR length(valid_until) BETWEEN 20 AND 64),
    created_by_actor_id TEXT NOT NULL CHECK (length(created_by_actor_id) BETWEEN 1 AND 128),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- The same two rules a memory carries, for the same reasons.
    CHECK (
        (source_kind IN ('user_statement', 'user_correction', 'provider_record')
            AND source_trust = 'authoritative')
        OR (source_kind IN ('tool_observation', 'document', 'model_inference')
            AND source_trust = 'derived')
        OR (source_kind = 'external_content' AND source_trust = 'untrusted')
    ),
    CHECK (source_kind <> 'model_inference' OR confidence = 'unverified'),
    -- An entity is not related to itself: "X works at X" is a data error, and allowing it would make a
    -- self-relation indistinguishable from a bug during a traversal.
    CHECK (subject_id <> object_id),
    -- A deleted relation retains no predicate text, matching the memory rule.
    CHECK (status <> 'deleted' OR length(predicate) = 0),
    CHECK (status = 'deleted' OR length(predicate) >= 1)
) STRICT;

-- Traversing outward from a subject, which is the direction a relation is usually read.
CREATE INDEX entity_relations_subject_idx
    ON entity_relations (workspace_id, subject_id, predicate);
-- ...and inward to an object, which is what "who is connected to this person" needs.
CREATE INDEX entity_relations_object_idx
    ON entity_relations (workspace_id, object_id, predicate);

CREATE TABLE memories (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    memory_type TEXT NOT NULL CHECK (
        memory_type IN (
            'working', 'conversation', 'episodic', 'semantic',
            'preference', 'relationship', 'procedural'
        )
    ),
    -- Useful text. `status = 'deleted'` is the only state in which this is empty, enforced by the pair of
    -- `CHECK`s at the end of the table: a deleted row cannot retain text, and a live row cannot be blank.
    content TEXT NOT NULL CHECK (length(content) <= 4096),
    -- The normalized subject/predicate/object claim, as **three columns rather than one JSON document**.
    --
    -- This follows the repository's own rule from `docs/data/schema.md`: "JSON stores versioned provider
    -- payload fragments or flexible metadata, not core relationships that need constraints." A claim triple
    -- is exactly a core relationship, so it gets columns — which also means a lookup by predicate is an index
    -- seek rather than a JSON scan, and a reader does not depend on a document's shape being stable.
    --
    -- All three are `NOT NULL` together or NULL together, enforced below: a claim with a subject and no
    -- predicate is not a claim.
    claim_subject TEXT CHECK (claim_subject IS NULL OR length(claim_subject) BETWEEN 1 AND 256),
    claim_predicate TEXT CHECK (claim_predicate IS NULL OR length(claim_predicate) BETWEEN 1 AND 256),
    claim_object TEXT CHECK (claim_object IS NULL OR length(claim_object) BETWEEN 1 AND 256),
    -- Provenance. All three `NOT NULL`: a claim with no source is not a memory, which is the architecture's
    -- central rule and therefore the schema's.
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
    -- The digest of the supporting excerpt, never the excerpt: storing the text would put a second copy of
    -- possibly-sensitive source content where deleting the source does not reach.
    source_excerpt_hash TEXT CHECK (
        source_excerpt_hash IS NULL
        OR (length(source_excerpt_hash) = 64 AND source_excerpt_hash NOT GLOB '*[^0-9a-f]*')
    ),
    confidence TEXT NOT NULL CHECK (
        confidence IN ('unverified', 'uncertain', 'likely', 'confirmed')
    ),
    importance INTEGER NOT NULL CHECK (importance BETWEEN 0 AND 4),
    sensitivity TEXT NOT NULL CHECK (
        sensitivity IN ('public', 'internal', 'confidential', 'restricted')
    ),
    -- The deduplication key: memory type, entity identifiers, and normalized content words. Derived, and
    -- cleared on deletion because it holds content-derived text.
    search_key TEXT CHECK (search_key IS NULL OR length(search_key) BETWEEN 1 AND 512),
    status TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'archived', 'deleted')),
    -- The validity window. `valid_until` absent means "until corrected"; the comparison is never done in
    -- SQL, because `UtcTimestamp`'s text form is not lexicographically sortable (see `0006_approvals.sql`).
    valid_from TEXT NOT NULL CHECK (length(valid_from) BETWEEN 20 AND 64),
    valid_until TEXT CHECK (valid_until IS NULL OR length(valid_until) BETWEEN 20 AND 64),
    -- Both supersession directions, because "is this claim still current" is on the retrieval path and
    -- following one direction would make it a scan of every later memory.
    supersedes_memory_id TEXT REFERENCES memories (id) ON DELETE SET NULL,
    superseded_by_memory_id TEXT REFERENCES memories (id) ON DELETE SET NULL,
    run_id TEXT REFERENCES agent_runs (id) ON DELETE SET NULL,
    created_by_actor_id TEXT NOT NULL CHECK (length(created_by_actor_id) BETWEEN 1 AND 128),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    created_at TEXT NOT NULL CHECK (length(created_at) BETWEEN 20 AND 64),
    updated_at TEXT NOT NULL CHECK (length(updated_at) BETWEEN 20 AND 64),
    -- When it was last selected into a context, and how often. Distinct from `updated_at`, so "when was this
    -- last edited" stays answerable from a row that is read often.
    last_accessed_at TEXT CHECK (
        last_accessed_at IS NULL OR length(last_accessed_at) BETWEEN 20 AND 64
    ),
    retrieval_count INTEGER NOT NULL DEFAULT 0 CHECK (retrieval_count >= 0),
    version INTEGER NOT NULL CHECK (version >= 1),
    -- **The injection boundary, in SQL.** `MemorySourceKind::permitted_trust` is an equality, so this
    -- restates it: external content cannot claim to be authoritative, and a provider record cannot claim to
    -- be a mere derivation. The domain refuses this at construction; the schema refuses it at write, so a
    -- row that arrived some other way is still refused.
    CHECK (
        (source_kind IN ('user_statement', 'user_correction', 'provider_record')
            AND source_trust = 'authoritative')
        OR (source_kind IN ('tool_observation', 'document', 'model_inference')
            AND source_trust = 'derived')
        OR (source_kind = 'external_content' AND source_trust = 'untrusted')
    ),
    -- A model inference cannot be anything but unverified: the model cannot assert a fact.
    CHECK (source_kind <> 'model_inference' OR confidence = 'unverified'),
    -- A provider is authoritative about what it recorded and not about a person's preference.
    CHECK (NOT (source_kind = 'provider_record' AND memory_type = 'preference')),
    -- Neither supersession direction may be a self-reference.
    CHECK (supersedes_memory_id IS NULL OR supersedes_memory_id <> id),
    CHECK (superseded_by_memory_id IS NULL OR superseded_by_memory_id <> id),
    -- **`deleted` is exactly when content is empty.** Two constraints because they say two things: a deleted
    -- row must not retain text, and a live row must not be blank. Together they make "status and content
    -- disagree" unstorable rather than something a reader has to defend against.
    CHECK (status <> 'deleted' OR (length(content) = 0 AND claim_subject IS NULL AND search_key IS NULL)),
    CHECK (status = 'deleted' OR length(content) >= 1),
    -- The claim triple moves together: all three or none, so a partly-populated claim is unrepresentable.
    CHECK (
        (claim_subject IS NULL AND claim_predicate IS NULL AND claim_object IS NULL)
        OR (claim_subject IS NOT NULL AND claim_predicate IS NOT NULL AND claim_object IS NOT NULL)
    ),
    -- A deleted memory names nothing as its replacement, because the trail was cleared with the text.
    CHECK (status <> 'deleted' OR superseded_by_memory_id IS NULL)
) STRICT;

-- **One memory per search key per workspace**, which is the admission lifecycle's "deduplicate / compare
-- existing" step made enforceable.
--
-- A duplicate is refused by the unique constraint rather than deduplicated in a read-then-write, which is
-- the same reasoning the tool-call idempotency ledger uses: a check followed by a write is a race, and the
-- index is not. The caller's response is to **reinforce** the existing memory (which is why
-- `retrieval_count` exists) rather than to insert a second row.
--
-- The key is cleared on deletion, and this index is why that is safe rather than a resurrection path: the
-- tombstone table below carries the key's **hash** and blocks the re-ingest. Without the tombstone, clearing
-- the key would let a re-ingest insert a fresh row for a claim the user deleted.
CREATE UNIQUE INDEX memories_search_key_idx ON memories (workspace_id, search_key);

-- Finding claims that share a predicate, which is how "does the user prefer X or Y" is answerable, and what a
-- single JSON document could not serve as an index seek. Partial, because most memories carry no claim.
CREATE INDEX memories_claim_idx
    ON memories (workspace_id, claim_predicate, claim_object)
    WHERE claim_predicate IS NOT NULL;

-- A run's memories, for the working and conversation types whose lifetime is the run or session.
CREATE INDEX memories_run_idx ON memories (run_id, created_at DESC);

-- The retrieval eligibility filter: a workspace's current claims of a given type, newest first. Every
-- predicate is a column this index covers, so the filter that runs before ranking is an index seek.
CREATE INDEX memories_eligibility_idx ON memories (workspace_id, status, memory_type, created_at DESC);

-- "What is this claim about", the other half of every eligibility query.
CREATE INDEX memories_entity_idx ON memories (workspace_id, created_at DESC);

-- The memory-to-entity link, with **how** the entity was matched.
--
-- `matched_by` is per link rather than per entity, because the same entity may be matched confidently in one
-- memory and guessed in another, and a reader deciding whether a claim is safe to state needs the answer for
-- the memory in hand.
CREATE TABLE memory_entities (
    memory_id TEXT NOT NULL REFERENCES memories (id) ON DELETE CASCADE,
    entity_id TEXT NOT NULL REFERENCES entities (id) ON DELETE CASCADE,
    matched_by TEXT NOT NULL CHECK (
        matched_by IN ('confirmed', 'provider_id', 'exact_identifier', 'probabilistic')
    ),
    PRIMARY KEY (memory_id, entity_id)
) STRICT;

-- "Which memories are about this entity", which is the query entity-scoped retrieval runs.
CREATE INDEX memory_entities_entity_idx ON memory_entities (entity_id);

-- **The tombstone: what blocks a deleted claim from coming back, without retaining what it said.**
--
-- `search_key_hash` is a SHA-256 of the deleted memory's `search_key`, so a re-ingest of the same observation
-- computes the same key, hashes it, and finds this row. The hash is one-way, so the words that were deleted
-- cannot be recovered from it — which is what lets the memory's own row satisfy "deleting it removes text and
-- derived indexes" while this row still refuses a resurrection.
--
-- `memory_id` is **not** a foreign key, and that is deliberate: a full user-deletion (`P4-008`) removes the
-- memory row, and the tombstone must outlive it. A cascade here would delete the evidence of the deletion.
--
-- The retained fields are the minimum an operator needs to answer "was this deleted, when, and by whom":
-- the identifier, the type (which tells them what kind of retention policy applied), the actor, the
-- correlation identity, and the instant. No content, no source locator, no entity list.
CREATE TABLE memory_tombstones (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
    memory_type TEXT NOT NULL CHECK (
        memory_type IN (
            'working', 'conversation', 'episodic', 'semantic',
            'preference', 'relationship', 'procedural'
        )
    ),
    search_key_hash TEXT NOT NULL CHECK (
        length(search_key_hash) = 64 AND search_key_hash NOT GLOB '*[^0-9a-f]*'
    ),
    -- The deleted memory's identifier, which survives the row it names.
    memory_id TEXT NOT NULL CHECK (length(memory_id) = 36),
    deleted_by_actor_id TEXT NOT NULL CHECK (length(deleted_by_actor_id) BETWEEN 1 AND 128),
    correlation_id TEXT NOT NULL CHECK (length(correlation_id) = 36),
    deleted_at TEXT NOT NULL CHECK (length(deleted_at) BETWEEN 20 AND 64)
) STRICT;

-- The resurrection check: one tombstone per claim per workspace, so a re-ingest finds one answer.
CREATE UNIQUE INDEX memory_tombstones_key_idx ON memory_tombstones (workspace_id, search_key_hash);

-- Answering "what was deleted here, and when", which is what an operator asks during a retention review.
CREATE INDEX memory_tombstones_workspace_idx ON memory_tombstones (workspace_id, deleted_at DESC);

UPDATE jarvis_storage_metadata
SET schema_version = 9
WHERE singleton = 1;

PRAGMA user_version = 9;
