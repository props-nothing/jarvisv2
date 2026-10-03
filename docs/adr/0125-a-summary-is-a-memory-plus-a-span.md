# ADR-0125: A session summary is a memory plus a span table, and its retention is an explicit verb

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P4-015`

## Context

`P4-015` states the requirement in one sentence: "a compressed summary of a session is stored as a `Derived`
claim with provenance and a retention rule, and is never presented as user-authored fact."

Three facts shaped the decision.

1. **A summary has two halves that different readers need.** It is a claim *about* a conversation — text, a
   source, a subject, a workspace, a sensitivity — and it is a compression *of* a range of turns. The first half
   is exactly a memory, and every rule that applies to a memory (workspace scoping, entity links, tombstones,
   supersession, retrieval) must apply to it. The second half is a fact no `memories` column can hold.

2. **`P4-008` recorded that no retention policy is implemented.** "the verbs exist and the sweeper does not, so
   nothing expires on its own." `MemoryType::is_durable` states the *rule* — `Working` and `Conversation` are
   non-durable, everything else survives its run — and had **no caller in production**: only a test named it.
   So the rule was documented, asserted, and unenforced.

3. **The slice's central prohibition is about attribution, not about text quality.** "Never presented as
   user-authored fact" is a claim about *how* a summary is stored and offered, not about whether the compression
   is good. Text quality is a model's job and is not checkable by a schema.

## Decision

**A summary is recorded as a `memories` row, and its span in a separate `session_summaries` table keyed by the
memory identifier.**

The memory half is produced by `SessionSummary::into_record`: `Conversation` type, `Document` source kind
(hence `Derived` trust), `Likely` confidence, a locator naming the session and the exact sequence range, and the
caller's conversation entity as its subject. None of those is supplyable by a caller — `SummarizeSessionRequest`
has no confidence, status, or trust field — because a caller able to send `authoritative` could present a
model's compression as the user's own words, which is the one outcome the slice forbids.

The span half is a table whose **primary key is the memory identifier**, so "this row is a summary" and "this
row has a span" are the same fact. It also stores `turns_covered` and `source_chars`, the loss metadata, because
they describe the **input** and the input changes: recomputing them at read time would measure a summary of
messages 1..4 against messages 1..400.

**The overlap rule and the span's membership rule are enforced in the writer, not the schema.**
`session_summaries` cannot see `messages`, so it cannot know that a span names turns the session does not have;
and SQLite cannot express "two intervals in one session must not intersect" without a trigger. Both are checked
in `record_summary`, which reads the session's summaries and its message count on the **same connection** as the
two inserts — a deferred transaction that read over the pool and then wrote failed with `SQLITE_BUSY_SNAPSHOT`
in `purge_memory` and would fail here for the same reason.

**The retention rule is an explicit verb, derived from the domain's own durability predicate.**
`archive_session_summaries` filters on a predicate **generated** from `MemoryType::is_durable` rather than a
hand-written list of type names, so the sweep and the rule cannot drift when a type is added. It is archive and
not delete: `MemoryStatus::Archived` is "retained for audit, not retrieved as current truth", which is exactly
the outcome wanted, and deleting would additionally require clearing the search key and writing a tombstone —
turning a retention decision into a **forget**, a different verb with a different receipt.

**A summary is offered to a model only when the turns it compressed are no longer replayed.**
`is_offerable_as_context(summary, replayed)` takes the spans the caller is replaying and answers whether the
summary is a duplicate of them. This is a rule about the **session**, not about the summary: it is false when
the summary is written and becomes true when the history window moves past it. A stored flag was rejected
because it would record a decision made once and read forever.

## Consequences

- The `memories` table gains no columns; the schema version moves to **12** for one new table and its index, so
  every existing row is untouched.
- **Two reads of a session's summaries have deliberately different filters.** `read_session_summaries` filters
  on `active`, because it answers "what does this session's summary currently say". `read_session_summary_spans`
  does not, because the overlap check must still see a set-aside span — otherwise archiving or removing a
  summary would re-open its turns to a second claim about the same passage. An audit path reads archived
  summaries through the memory read, which includes archived rows.
- **Three rules now have production callers that had none**: `is_durable` (the sweep), `spans_overlap` (the
  overlap check and the offerability predicate), and `MemoryType::Conversation`'s "session retention applies".
- **A summary's memory follows the ordinary duplicate rule**, because its search key is derived from its text
  and its subject. Two summaries of *different* spans that say the same words collide, and the refusal is a
  `422` naming the remedy rather than a `503` telling a caller to retry. A caller that wants both keeps the span
  rather than the text.
- **Recorded as a limit:** the offerability rule is checked against the spans a caller is *replaying*, and no
  component yet moves a history window past a span — so the predicate's `true` branch is reachable only from a
  caller that supplies non-overlapping spans. The rule is complete and its falsification is asserted; what is
  absent is a windowing component, which no slice in `TODO.md` currently specifies.
- **Recorded as a limit:** a summary whose memory row is deleted by a tombstone write is not re-checked against
  `memory_tombstones`. A summary is not re-ingested from a source, so the resurrection rule has nothing to
  guard; `record_memory` keeps it for the paths that do re-ingest.
