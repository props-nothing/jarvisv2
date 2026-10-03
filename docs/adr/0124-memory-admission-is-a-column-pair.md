# ADR-0124: A memory's admission is a column pair, and its cross-column rule lives in the decode

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P4-014`

## Context

`docs/architecture/memory-and-context.md` ends its admission lifecycle in "store / supersede / propose
review", and `P4-014` states the requirement plainly: "admission is a decision that names its approver".

The state half existed. `MemoryStatus` has had `proposed -> active` since `P4-002`, and `MemoryRecord::confirm`
performs it. What did not exist was the decision: `confirm` took no identity and the `memories` table had no
column to hold one. So a confirmed claim could be read back as current with **nobody behind it**, and "who
decided this is true" was unanswerable from stored data.

Two further facts shaped the decision:

1. **No route called `confirm` at all.** `MemoryTransition::Confirm` was written in `P4-002` and reachable from
   no HTTP surface, so a `Proposed` claim — which the daemon *can* create, since `relationship` and
   `model_inference` both derive that status — could never be accepted. A claim the workspace held, offered to
   nobody, with no way to make it current.
2. **`skill_revisions` already had the column pair** (`promoted_by_actor_id` / `promoted_at`) for exactly this
   requirement, from `ADR-0117` §4.

## Decision

### 1. A column pair on `memories`, not a decision log

`0011_memory_admission.sql` adds `admitted_by_actor_id` and `admitted_at`, with the equality rule
`(admitted_by_actor_id IS NULL) = (admitted_at IS NULL)` expressed as the domain's decode check rather than as
SQL — see decision 3.

A separate decision table was rejected for the reason `ADR-0117` §4 gives: "is this claim trusted, and on whose
authority" is asked by the **presentation** path on every retrieval for an individual memory, not by an audit
query over a log. A log makes it a join on the hot path and gives the two answers two places to disagree; a
column makes "a confirmed claim with nobody behind it" the unrepresentable case.

### 2. The backfill is empty, deliberately

Rows that predate this migration were confirmed by a route that recorded no approver. There is no value to
migrate, and every candidate would be a fabrication: the local user, or the row's `created_by_actor_id`. A null
means exactly what it should — *this claim's acceptance was not attributed* — and inventing an authorization for
a decision nobody made is worse than an honest gap.

### 3. The cross-column rule is enforced on the read path, and that is the only enforcer

The rule that matters is a relationship between three columns: an `active` (or `archived`) row may name an
approver, a `proposed` one may not, and the two columns move together.

**SQLite cannot express it here.** `ALTER TABLE ADD COLUMN` accepts only a `column-def`, so a `CHECK` mentioning
`status` is rejected as soon as it is added — confirmed against the official `lang_altertable.html`, whose
"Making Other Kinds Of Table Schema Changes" section is the twelve-step rebuild. For `memories` that rebuild
would recreate four indexes and two self-referencing foreign keys (`supersedes_memory_id`,
`superseded_by_memory_id`) and copy every row, which is disproportionate for a rule the decoder can hold.

So `MemoryRecord::from_stored` enforces it, beside the status-and-content rule this table already relies on
(`deleted` is exactly when the content is empty — a rule that is *also* not expressible as an additive
constraint, and for the same reason).

**Recorded as a limit:** a `proposed` row written directly into the database can carry an approver until
something reads it. No code path in this build can produce one — nothing sets either column except
`confirm_by`, which sets the status in the same value — so reaching that state requires another build, a
restored backup, or a hand edit, which is exactly what the decode check is for.

### 4. The approver is derived, never supplied

`ConfirmMemoryRequest` carries only `expected_version`. The approver is read from the daemon's seeded identity,
as the author is.

This is the one place the guard the requirement implies actually holds: the model can request *tools* and never
call a route, so it cannot name itself as the approver of anything.

### 5. `confirm` is kept, as the decode companion

`confirm(at)` remains and records no approver. It is reachable only from a decode and from the storage layer,
and its job is to re-apply a **stored** status without inventing the decision that produced it: a row written
before admission decodes to `active` with no approver, and a decode that called `confirm_by` would have to
fabricate one.

`MemoryTransition::Confirm { approver_actor_id }` carries the approver **inside the variant** rather than as an
argument beside the transition, so a state change cannot be separated from the justification for it.

## Consequences

- **The requirement is satisfied where it can be.** "Admission is a decision that names its approver" is now
  true of every admission this build can perform, and the pair is durable rather than a reply field.
- **A correction keeps the admission.** Correcting a claim archives it, and an archive yields `None` for both
  fields — so the write takes them from the record, not from the transition. Writing `None` would erase the
  record of who accepted a claim at the moment it was superseded, which is when an audit would want it.
- **A corrupt row now answers `500`, not `422`.** `decode_memory` reports `StoredMemoryInvalid` rather than
  `InvalidMemoryRequest`, because the value came from the row and not from an argument. A caller told to change
  their request when what they need is a backup has been sent the wrong way, and the field-name table is now
  shared by both mappings rather than duplicated.
- **One guard was written and then deleted, and that is the most useful outcome of the slice.** A self-admission
  refusal mirroring `ADR-0117` §4 was implemented, falsified, and removed: `memory-and-context.md` says a
  high-impact inference "requires explicit user confirmation", so the person confirming **is** the person whose
  statement produced the candidate. With one seeded identity the author and approver are always the same value,
  and the guard refused every legitimate confirmation. The rule worth wanting — *an agent must not admit what it
  authored* — needs an actor vocabulary that distinguishes a model from a person, which this build does not
  have. Two tests now assert the **non**-refusal so the symmetry cannot be restored without seeing why.

## Honest limits

- **The model still has no submission path.** No tool touches memory, so "the model may submit candidates" is
  not yet true — and that is what would give the self-admission rule a subject. `P4-014`'s remaining work.
- **The read path is the only enforcer of the cross-column rule.** See decision 3.
- **An admission cannot be withdrawn or superseded as an admission.** A later acceptance is refused because the
  claim is no longer a proposal, so "who confirmed this, and has anyone changed their mind" is one row's worth
  of answer rather than a history.
- **Sub-second ordering is still unresolved**, so a listing of two admissions in one second orders by `id`.
  `ADR-0034`'s decision remains open.
