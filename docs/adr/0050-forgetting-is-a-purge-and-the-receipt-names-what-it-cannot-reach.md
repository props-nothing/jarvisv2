# ADR-0050: Forgetting is a purge that writes a tombstone first, and the receipt names what it cannot reach

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-008` gives the memory surface a user-facing lifecycle: inspect, search, remember, correct, forget, export,
and retention. `docs/api/contracts.md` sketches the verbs and a `DeletionReceipt`; `docs/architecture/memory-and-context.md`
states the rules they have to satisfy — "deleting it removes text and derived indexes", retention and export
per type, provider-side copies "surfaced separately", and a user able to ask *why* something was remembered or
used.

The domain, the storage layer, the candidate pipeline, the retrieval, and the embedding port already existed
and were tested, and **nothing could reach any of them**: no route, no CLI verb, no reader. So this slice is
mostly wiring, and the decisions below are the places where wiring forced a choice.

Four of them were already settled elsewhere and are only cited here: `ADR-0044` (a deleted memory stores a
hash of its key), `ADR-0045` (a correction is declared), `ADR-0046` (eligibility is a filter and the
explanation is the arithmetic), and `ADR-0049` (retrieved content is fenced). What follows is what was not.

## Decision

### 1. A full user deletion is a different operation from the `Delete` transition, and both are kept

`apply_memory_transition`'s `Delete` clears the **text** and keeps the row: the status becomes `deleted`, the
content and claim triple are emptied, the search key is nulled, and the row, its actor, its correlation
identity, and its provenance remain. That form exists because a source link has to keep resolving and the audit
trail has to survive.

`purge_memory` is the stronger request — "remove this and do not keep a record that says anything" — and it
**deletes the row**. `memory_entities` cascades; `supersedes_memory_id` and `superseded_by_memory_id` are
`ON DELETE SET NULL`, so a claim that pointed at this one stops pointing at a row that no longer exists.

Two operations rather than one with a flag, because they are answers to different questions and the receipts
differ: `Delete` leaves a row a source link resolves against, while a purge leaves only a tombstone. Collapsing
them would make "delete" mean whichever the caller intended, and the one it did not mean would be unrequestable.

### 2. The tombstone is written **before** the delete, so the failure direction over-blocks

The tombstone is the only durable record that blocks a re-ingest, and it deliberately has **no** foreign key to
the memory so it can outlive it (`ADR-0044`). Writing it first means a crash between the two statements leaves a
tombstone for a memory that still exists, which the user can resolve by deleting again. The reverse order would
leave a deleted memory with no tombstone, so a later ingest would resurrect a claim the user removed.

The safe failure direction is the one that **over-blocks**. A deleted claim that cannot be re-learned without an
explicit request is a nuisance; a deleted claim that returns is a broken promise.

### 3. The tombstone must be written on the purge's own connection, and the signature enforces it

This is a correctness constraint rather than a preference. SQLite in WAL mode fails a **deferred** transaction
that reads and then writes with `SQLITE_BUSY_SNAPSHOT` if another connection committed in between. The first
implementation wrote the tombstone through the pool — a second connection — while the purge's transaction held
a read, and the delete then failed with a bare "failed to purge a memory".

So `write_tombstone_on` takes a `&mut Transaction<'_, Sqlite>` rather than the database, which makes the wrong
version a **compile** error rather than a subtle runtime one. It also means the row's fields come from the
transaction's own `SELECT`, so the type and provenance written into the tombstone are the ones the delete acts
on.

### 4. `allow_relearn` is an explicit request, and it removes the tombstone rather than skipping the write

The default is `false`. Asking for `true` is a deliberate statement that the user wants the claim to be
re-learnable — an **undo** of a deletion rather than a cleanup — so it removes any existing tombstone *and*
skips writing a new one. The receipt reports which of the two happened, so the audit trail distinguishes them.

The same reasoning as `ADR-0017`: a safe default plus an explicit, recorded request. A
flag that merely suppressed a write would leave an older tombstone in place, so the undo would appear to work
and then not.

### 5. `expected_version` is required on correct and forget, and that is the opposite of `ADR-0022`

`ADR-0022` decided that a run cancellation carries no version, because a run's own terminal-state rule already
refuses a second cancellation and a version would add nothing but a refusal. Correct and forget are different:
they change **what will be retrieved as current truth**, and there is exactly one right subject for the write —
the claim the user was looking at. A correction applied to a claim the user has not read is a change to their
memory they did not ask for.

The version is therefore **on the reference** and not only on a write's reply. The caller obtains an expectation
by reading; a reply that omitted it would leave a client re-reading and hoping, which is the lost update the
guard exists to prevent. And its absence from the CLI is a **usage error** rather than an implicit re-read: a
client that did not read the claim has no basis for overwriting what the user currently believes, and fetching
a version silently would apply the correction to whatever the claim says *now*.

Two staleness reasons exist and both are tested separately, because a guard tested only against the first
accepts the second:

- a version the caller **never saw**; and
- a version the caller saw **before a correction archived the claim**, which advanced that claim's own version.

### 6. A search hit carries version `0`, and a purge guard refuses it

The ranking holds a `MemoryRecord`, which does not carry the optimistic-concurrency version, so `hit_of` can
only fabricate a value. It reports `0`, and versions start at one, so `0` **cannot** match a stored version: a
client that passed it would be refused rather than performing a write. That makes the marker safe as "I do not
know" instead of dangerous as a plausible guess.

The consequence is recorded as a limit rather than hidden: a search result cannot be corrected or deleted
without a `memory show` first. That is the honest shape — fabricating a version would be worse than an absent
one, because a caller would present it and have a valid write refused.

### 7. No scope in a request body, and `deny_unknown_fields` is what makes it structural

Every read and write takes the workspace from a **loaded** `LocalIdentity`, never from a request. The DTOs use
`deny_unknown_fields`, so a `workspace_id` in a body is a `422` naming the field rather than an ignored value.
That makes "client A's memory cannot enter client B's context" a property of the transport rather than a check
each handler has to remember — the same rule the tool-call body's absent `workspace_id` follows.

The read-side check is a **comparison after the read** rather than a `WHERE` clause, because the two failures
need different answers: "no such claim" and "not in your workspace" are the same thing to a caller not entitled
to know the difference. A claim in another workspace reports `404`, never `403`, because "not yours" would
confirm that something exists.

### 8. A reference never carries content; `show` and `export` are the two exceptions, for different reasons

A listing, a search hit, and a write's reply identify a claim by identifier, type, source, standing, and
timestamps. Nothing else. Retrieval's own contract is that a context item carries an opaque reference rather
than text, so a listing that returned content would be a second, less careful path into a prompt.

`GET /memories/{id}` returns the text because a user asking to see one claim is the case where content **is**
the answer. `GET /memories/export` returns it because `docs/architecture/memory-and-context.md` requires
"configurable retention and export behavior", and an export whose text was redacted would not be one — that is
the `GDPR`-shaped portability case.

The export also states what it **excludes**, in a list, because each entry is a different reason a reader might
otherwise assume completeness. An archive that looks complete and is not is worse than one that says what it
left out.

### 9. An empty entity list is refused rather than filled with a placeholder

A memory must be *about* something: the domain refuses one with no entity, and the entity is what makes "what
do I know about this person" an index seek rather than a text search. The convenient alternative is to attach a
placeholder subject, and it is wrong for the reason `P4-001` records: a claim attached to a placeholder *looks*
resolved, so a later question about the person it is really about will not find it and nothing in the store says
why. Refusing with the field named is a refusal the caller can act on; inventing the entities from the text
would be entity extraction, which no slice has built.

The consequence is a recorded limit: **there is no entity resolution yet**, so a remember must name an entity
identifier the caller already holds.

### 10. A duplicate is caught by the unique index, and the reply reports that rather than the pipeline's decision

The pipeline's comparison against an existing memory decides `Duplicate` — but only when it is given one, and a
remember is a *new* claim, so it supplies no `existing`. The pipeline therefore decides `New`, and the unique
index on `(workspace_id, search_key)` is what catches a re-statement.

The first implementation let that error escape, so a user restating a claim they had already made was told "the
memory was changed by another writer; re-read it and retry" — a message about a race for an ordinary
re-statement. The fix reports the existing memory and returns `200` rather than `201`.

It also threads a `wrote: bool` out of the store, because the pipeline's name (`New`) and the fact of a write
are **different answers**. A reply built from the admission alone reports "remembered" for a request that stored
nothing, and the outcome and the status code would be wrong in the same direction. Pairing them makes the
divergence unrepresentable.

The existing memory is **not** reinforced: reinforcement counts a retrieval, and this request did not cause one.

### 11. A correction's window opens when it is recorded, because the domain refuses a backdated claim

A memory's `valid_from` may not precede its `created_at`. A window that opens before the record existed is a
backdated claim, and it is how a later correction would fail to outrank the thing it corrects. So a correction
**cannot** inherit the original's `valid_from`: carrying it onto a row created now is exactly that backdating,
and the first implementation did it — the domain refused with a message about a window the caller never
supplied.

A correction therefore takes its `valid_from` from its own creation instant, and carries the original's
`valid_until` over unchanged: a correction restates what a fact *says*, it does not extend a fact meant to
lapse.

That leaves a real limit, recorded rather than papered over: correcting a claim that had not yet taken effect
makes the correction effective from now rather than from the original's start, so the pair reads as
overlapping. And a claim whose window has **already** closed cannot be corrected at all — the correction would
be born expired, which the domain cannot express — so it is refused with `lapsed` and the caller is told to
record a new claim instead.

### 12. Deletion is reported as a **receipt**, and its `unreachable` list is never empty

`docs/api/contracts.md` names the type `DeletionReceipt` and the name is the point: it is the evidence a
privacy obligation was met. It counts removals by **category** — characters removed, whether the search key
went, whether a tombstone was written, how many entity links went, whether a supersession link was cleared —
and a length rather than the text, because a receipt that quoted the removed content would be the content
surviving the deletion in a second table.

`unreachable` is **never empty**. There is always at least one scope outside this platform's reach: a provider
that received the claim as context holds its own copy under its own retention policy, and a backup taken before
the deletion may still hold it. `docs/architecture/memory-and-context.md` requires that be "surfaced
separately" rather than implied, so a receipt that read as total would be misleading at exactly the moment it
is supposed to inform.

## Consequences

- **The CLI verbs are thin on purpose.** Each turns arguments into a request and renders a reply; none decides
  whether a claim is acceptable, what confidence it supports, or whether a correction should apply. A client
  that re-derived those would be a second implementation of a rule that already exists in one place. The
  `expected_version` a correction carries is passed through rather than managed, because the caller is the party
  that read the claim.
- **The refusals name the field a caller can fix**, and `MemoryServiceError` has two renderings: `detail()` for a
  client and `Display` for a log. The same string reaching both destinations is how a database path ends up in
  an API response, so a `DatabaseError`'s text is never forwarded.
- **A tombstone is mapped to a refusal rather than to `Storage`.** The durable check lives in storage rather than
  in the pipeline's `tombstoned` stage because only storage can answer the question atomically; the first
  mapping fell through to `Storage`, so a client that re-stated a deleted claim was told "the local database is
  not available".
- **The reads load entity links.** Three of them decoded the row without joining `memory_entities`, so a
  correction inheriting the original's entities inherited **nothing** and was refused for naming no entity — a
  message about the caller's request for a defect in a read. A missing join is invisible: the row decodes, every
  field is present and typed, and the empty vec looks like a legitimate "about nothing".
- **The export's exclusion text was wrong and was corrected.** It said "deleted claims appear as an empty
  record", which describes the `Delete` **transition**; the only deletion verb this surface has purges. The doc
  and the text agreed with each other while both described a path no verb takes, which is what an assertion on
  the *observable* absence found.
- **Recorded limits.** No entity resolution, so a remember must name an entity. `importance` defaults to `2`,
  the middle of the range, so a default cannot outrank explicit user statements. No retention policy is
  implemented: the verbs exist and the sweeper does not, so nothing expires on its own. No provider-side
  deletion. A search hit cannot be corrected without a `show`.
