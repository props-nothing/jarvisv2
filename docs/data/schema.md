# Conceptual Data Schema

This document defines ownership, keys, and invariants. SQL migrations are the executable source after implementation begins. Backend-specific types may differ while preserving behavior.

## Conventions

- Primary IDs are application-generated UUIDv7.
- Timestamps are UTC; user IANA timezone is stored separately.
- Workspace-owned rows include `workspace_id` and repositories require it explicitly.
- Mutable aggregate roots include `version` for optimistic concurrency.
- External identifiers are unique only within provider/account scope.
- JSON stores versioned provider payload fragments or flexible metadata, not core relationships that need constraints.
- Secret values never appear in ordinary tables; store `secret_ref_id`.
- Audit/event payloads are bounded and sensitivity-labelled.

### `sensitivity` Vocabulary

The allowed values are defined by `jarvis_core::Sensitivity`, which is the single
definition for storage, wire contracts, and policy:

| Value | Level | Meaning |
| --- | --- | --- |
| `public` | 0 | Safe to disclose broadly, including outside the machine. |
| `internal` | 1 | Normal private content for the owning workspace. **This is the default.** |
| `confidential` | 2 | Exposure would be harmful, for example personal documents. |
| `restricted` | 3 | Requires explicit handling, for example credentials or health data. |

This is a **flow policy**, not a label. The levels are ordered so content may flow
outward only to a destination at least as restrictive: `can_flow_to` returns false
when content is more sensitive than its destination. The default is `internal`, not
`public`, so a row whose classification was never set cannot be treated as safe to
disclose. Storage constraints use the same four spellings, so a value that the domain
rejects cannot be persisted through a direct SQL write.

## Identity

### `users`

`id`, display/profile fields, locale, timezone, status, created/updated timestamps.

### `workspaces`

`id`, name, mode, data policy, default model/runtime policy, status, version, timestamps.

### `workspace_memberships`

`workspace_id`, `user_id`, role/capability set, status, created/revoked timestamps. Unique per user/workspace.

### `clients`

Represents CLI, desktop, browser, mobile, MCP, voice, runtime, or service clients. Includes client type, public-key/credential reference, trust level, last seen, expiry/revocation, and metadata.

### `client_grants`

Client/workspace capability scopes, resource constraints, policy ceiling, validity, issuer, and revocation.

## Conversations And Runs

### `sessions`

`id`, workspace/user/channel, title, status, active runtime/model policy, created/updated/archived timestamps, version.

### `messages`

Session, author kind/ID, role, ordered sequence, content envelope, sensitivity, source, provider message reference, timestamps. Unique sequence per session.

### `agent_runs`

Session/workspace/user, objective, state, selected runtime, runtime binding reference, context-manifest reference, cancellation state (`cancellation_requested_at`), terminal outcome, error code, usage/cost summary, version, timestamps.

`state` is constrained to the states of the native run state machine and terminal states
are constrained to carry an outcome. The **legal transitions** between states are not
expressible as a per-row `CHECK` (they depend on the previous row) and are therefore owned
by `jarvis_core`, with the repository enforcing them by comparing the caller's expected
state and version in the writing statement. A legal transition is refused when the stored
state or version has moved, so two writers cannot both advance one run.

### `agent_steps`

Run, stable step key, attempt group, kind, state, summarized input/output references, start/end timestamps, error/outcome, version. Do not store hidden chain-of-thought.

### `model_calls`

Run/step, provider/model, request policy, prompt/context hashes, capability requirements, attempt, streaming flag, status, latency/TTFT, normalized usage/cost, provider request ID, redacted error, timestamps.

Usage is recorded here per call and summarized on `agent_runs`. There is no separate
usage table: a duplicated total could disagree with the parts it summarizes.

### `run_events`

Run, per-run monotonic `sequence`, canonical event kind, bounded operational summary,
bounded JSON payload, correlation ID, occurred/recorded timestamps. Unique on
`(run_id, sequence)`. Append-only.

The durable source for the run activity/output stream. `sequence` is scoped to one run and
starts at 1, and the writer allocates it inside the insert so a stream cannot have a gap or
a duplicate. A terminal event is always the last event of its run, which is verified on
read because that is a property of the stream rather than of one row.

This is **not** the Phase 6 `events` / `event_inbox` / `event_outbox` bus. Those are
cross-aggregate records with lease, attempt, and dead-letter semantics; a run stream needs
per-run total ordering and replay for one watching client. See
[ADR-0011](../adr/0011-run-events-and-http-transport.md).

### `artifacts`

Workspace/run/tool/workflow ownership, object key, media type, size, hash, sensitivity, retention, origin, created/deleted timestamps.

## Tools, Policy, And Approvals

### `tool_definitions`

Stable tool ID/version, source, schemas, effects, baseline risk, scopes, timeout/retry/idempotency metadata, active status, definition hash.

Runs should retain the definition version/hash used even after registry updates.

### `tool_calls`

Run/step/workflow, tool/version, actor/client/workspace/account, canonical intent hash, bounded input/output references, effect/risk, state/outcome, idempotency key, provider receipt, ambiguity marker, timestamps, version.

### `policy_decisions`

Tool call/request, policy version/hash, inputs summary, decision, reason codes, obligations (approval/sandbox/redaction), evaluator version, timestamp. Append-only.

### `approvals`

Request/tool/workflow, intent hash, display snapshot, eligible approver constraints, required auth strength, status, nonce hash, expiry, decision actor/client/channel, reason, timestamps, version.

An approval can resolve only one unchanged intent and cannot move from a terminal state.

## Connectors And Secrets

### `connector_definitions`

Connector ID/version, manifest hash, provider, auth methods, supported operations, docs/research version, status.

### `connector_accounts`

Workspace, connector, verified provider account ID, display identity, secret reference, granted/requested scopes, status, health, token expiry metadata, timestamps, version. Unique provider account per workspace/connector unless multi-profile policy allows otherwise.

### `connector_cursors`

Account and stream/resource key, opaque cursor, cursor version, last successful sync, status, version.

### `webhook_subscriptions`

Account/provider subscription IDs, endpoint identity, secret reference, resource/filter, expiry/renewal, status, version.

### `secret_references`

Logical secret ID, provider/store, owner scope, purpose, version/rotation timestamps, health metadata. No secret value.

## Memory And Knowledge

### `memories`

Fields described in [memory-and-context.md](../architecture/memory-and-context.md), including type, content, structured claim, source, confidence, importance, sensitivity, validity, status, supersession, actor, embedding metadata, and timestamps.

### `memory_embeddings`

Memory, embedding model/version/dimensions, input hash, vector/blob, created timestamp. PostgreSQL uses pgvector; SQLite may use a selected local extension or an application-managed index. Rebuildable.

### `entities`

Workspace, kind, canonical label, normalized attributes, confidence/status, timestamps, version.

### `entity_aliases`

Entity, alias kind/value, provider/account source, verification state, confidence, uniqueness scope.

### `entity_relations`

Workspace, subject, predicate, object, source, confidence, validity, status, timestamps.

### `documents`

Workspace, title/type, source, object key, content hash, sensitivity, indexing state, retention, timestamps.

### `document_chunks`

Document, stable ordinal/location, text or object reference, hash, token estimate, metadata. Derived and rebuildable.

### `document_embeddings`

Chunk plus embedding metadata/vector. Derived and rebuildable.

### `context_manifests`

Run/model call, selected source references, priorities, token estimates, inclusion reasons, sensitivity routing, compaction metadata. Store full prompt only under explicit sensitive-debug policy.

## Events And Workflows

### `events`

Canonical event envelope: type/schema, source/external ID, workspace/actor, times, causation/correlation/trace, dedupe key, sensitivity, bounded payload, verification evidence.

Unique on provider/source/external event identity or workspace/dedupe key as appropriate.

### `event_inbox`

Ingress event, processing state, lease owner/fence/expiry, attempts, next attempt, terminal/dead-letter reason, timestamps.

### `event_outbox`

Aggregate type/ID/version, event, destination/topic, state, lease/attempts, timestamps. Inserted transactionally with aggregate changes.

### `workflow_definitions`

Workspace/global scope, stable name, version, schema, definition, enabled status, trigger/policy metadata, timestamps. Immutable by name/version.

### `workflow_runs`

Definition/version, initiating event/actor, state, input/output refs, current wait, cancellation, timestamps, version.

### `workflow_steps`

Run, step ID, state, dependency/join metadata, effect/idempotency, attempts, wait/timeout, output/error, version.

### `workflow_attempts`

Step, attempt number, lease/fence, started/ended, result class, external receipt, error, heartbeat. Append-only.

### `schedules`

Workspace, workflow/command target, schedule expression/type, IANA timezone, DST/missed-run policy, next/last run, status, version.

## Notifications And Voice

### `notifications`

Workspace/user, source event/run/workflow, channel, priority, dedupe key, content/artifact refs, state, provider receipt, timestamps.

### `voice_sessions`

Provider/channel, provider session/conversation, resolved user/workspace, identity evidence, linked session/run, capabilities, language/voice, turn-detection policy (mode, thresholds, max-silence cap, backchannel suppression) and its recorded effective provider values, interruption policy (sensitivity, input-during-speech reporting, resume timeout), retention/consent policy, state, timestamps, version.

### `voice_turn_events`

Voice session, canonical turn-event kind (`end_of_turn`, `eager_end_of_turn`, `turn_resumed`, `false_interruption`, `backchannel`), provider-reported confidence and its interpretation, detected/interrupted/resumed timestamps, speculative model calls discarded, turn-detection latency, provider event reference, sequence, version. Append-only.

The `end_of_turn` row is the canonical transcript boundary. Confidence is retained as
provider evidence and is not compared across providers.

### `voice_calls`

Voice session, provider/call IDs, direction, from/to normalized references, reason, initiating event/workflow, policy/approval receipt, state, costs, termination reason, transcript/audio refs, timestamps, version.

### `webhook_deliveries`

Provider/endpoint, event ID/type, body hash, signature result, received/responded time, HTTP result, dedupe result, processing state. Raw bodies are not retained by default.

## Runtime Management

### `runtime_definitions`

Runtime ID/version, protocol range, launch/remote configuration, capability manifest, trust/sandbox policy, status.

### `runtime_instances`

Definition, instance identity, process/endpoint metadata, health, protocol/capabilities, started/last heartbeat/stopped timestamps, failure count.

### `runtime_bindings`

JARVIS run/session, runtime instance/definition, opaque native session reference, adapter version, status, timestamps. Opaque data is encrypted/classified when sensitive.

## Audit

### `audit_records`

Append-only actor/client/workspace, action type, target type/ID, decision/outcome, policy/tool/config versions, request/result hashes, correlation/trace IDs, sensitivity, safe metadata, timestamp, integrity metadata.

Audit content is intentionally less detailed than operational tables. It must remain useful after sensitive content deletion without becoming a hidden copy of that content.

## Phase 1 Minimum

Do not create every table in the first migration. Phase 1 needs:

- `jarvis_storage_metadata` (implemented by `0001_initialize.sql`)
- `daemon_instances` (implemented by `0002_daemon_instances.sql`)

The remaining Phase 1 tables are added only with the behavior that owns them;
their absence from the initial storage migration is intentional.

Two tables were previously listed here as Phase 1 requirements and are **not**
created by any migration: `config_metadata` and `audit_records`.

- `config_metadata` is unnecessary. Configuration is a versioned TOML document
  owned by `ConfigStore`, and the file itself carries the version, so a database
  table would be a second copy of a value the config already owns. Configuration is
  also readable before the database is opened, which is what lets `doctor` diagnose a
  broken database.
- `audit_records` is a **Phase 3** concern rather than Phase 1. It becomes
  meaningful with the tool-effect pipeline it must record, and creating an
  append-only audit table with no writer would prove nothing while inviting
  half-populated rows.

Recording this here because the earlier list read as an unmet requirement. A doc that
names a required table that no migration creates is drift, and it is indistinguishable
from an unimplemented obligation until someone checks.

### Implemented Migrations

| Version | File | Adds |
| --- | --- | --- |
| 1 | `0001_initialize.sql` | `jarvis_storage_metadata` |
| 2 | `0002_daemon_instances.sql` | `daemon_instances` |
| 3 | `0003_conversation_state.sql` | `users`, `workspaces`, `sessions`, `messages`, `agent_runs`, `agent_steps`, `model_calls` |
| 4 | `0004_run_events.sql` | `run_events` |

Two decisions in migration 3 are worth recording because they are structural, not
typographical:

- **Usage is not a separate table.** A run-level summary lives on `agent_runs` and the
  per-call detail lives on `model_calls`. A third table holding the total would be able
  to disagree with its own parts.
- **Legal run transitions are enforced in Rust, not by a trigger.** A trigger would need
  the previous row to decide, and it would be invisible to the application. The
  migration constrains the *states*; `P2-005` owns the transition table. The CHECK
  constraints do make inconsistency unrepresentable: a terminal `agent_runs` row must
  carry an outcome and an end time, and a failed `model_calls` row must carry a redacted
  error code, so "failed but still running" cannot be stored.

Two further details are load-bearing and easy to get wrong later:

- **`agent_runs` is declared before `messages`, not after.** `messages.run_id`
  references `agent_runs`, and because `run_id` is nullable, a NULL value skips
  foreign-key checking entirely. A forward reference would therefore pass every test
  that stored a message without a run and fail only for the first writer that set one.
  Parent-before-child removes the failure mode rather than testing for it.
- **`content_bytes` is checked against `length(CAST(content AS BLOB))`, not
  `length(content)`.** SQLite's `length()` returns CHARACTERS for a TEXT value and
  BYTES for a BLOB. `jarvis_models` budgets UTF-8 bytes, so the uncased form would have
  enforced a character count and rejected every message containing non-ASCII text.

Phase 2 adds users/workspaces/sessions/messages/runs/steps/model calls. Later phases add their bounded groups with migrations and repository tests.

## Required Cross-Backend Tests

- constraints and unique/idempotency behavior
- optimistic concurrency conflicts
- transactional outbox atomicity
- lease acquisition/expiry/fencing
- migration from prior schema and unsupported-newer-schema refusal
- backup and restore
- workspace filtering
- deletion and derived-index cleanup
- equivalent ordering/pagination semantics