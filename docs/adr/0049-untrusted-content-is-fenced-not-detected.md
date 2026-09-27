# ADR-0049: Untrusted content is fenced, and the fence is not a promise the model will obey

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-007` integrates retrieval into the request a model is given. `docs/architecture/memory-and-context.md`
requires assembly step 5 to "mark external content as untrusted data and isolate instructions found inside
it", and `docs/architecture/security.md` treats retrieved content as data rather than authority.

Two existing mechanisms already covered part of that:

- `jarvis_core::context` decides **whether** untrusted content may be included, and refuses an untrusted item
  that is not marked `quoted`.
- `jarvis_core::memory` decides **what trust a stored claim carries**, from its origin, and refuses an
  `ExternalContent` memory above `Untrusted`.

Neither decides **how** untrusted text is presented once it is included. That gap matters because the model
receives one flat string: the marking is a decision recorded *beside* the content, not a property *of* it.
An instruction inside a retrieved memory arrives at the model as ordinary text in the user turn it was
appended to.

So the work was to close that gap, and the first thing it forced was a decision about what "isolate
instructions" can honestly mean.

## Decision

### 1. Isolation is not detection, and the module says so

"Detect and remove instructions" is not implementable. Deciding whether a sentence is an instruction requires
understanding it, and a classifier that could do that reliably would be the same class of model whose
behaviour is at risk. Every pattern list is defeated by rephrasing, by a language the list does not cover, or
by text the list's author never imagined.

So the module does not detect and does not claim to. It makes the text have three properties instead:

### 2. Three properties, each of which an attacker cannot undo

**Neutralised characters.** Bidirectional overrides, zero-width joiners, directional marks and the other
format characters change how text *reads* without changing what it *is*. In a prompt where the model reads a
transcript, text that displays one thing and encodes another is a deception primitive rather than a
typographic nicety. They are removed. `jarvis-mcp` already applies the same range set to a tool's own
description, and the duplication is recorded as a cost: the two lists must change together, and each crate
has a test over the ranges.

**A fence that cannot be closed from inside.** The payload is delimited by a boundary token, and any
occurrence of that token **in the payload** is removed — so the text cannot end its own quarantine and
continue as though it were outside. Removal rather than escaping, because escaping would need the model to
understand the escape while removal needs nothing of it. The check is case-insensitive and separator-tolerant,
because the token is made of ordinary letters and hyphens and a payload writing it in lower case reads to a
model as the same marker.

**A stated framing that this platform authored.** The instruction that the region is data is written by
JARVIS and placed before the payload, so the model reads what the region *is* before its contents.

Property 3 is why the fence is **not** a security claim. The claim is properties 1 and 2: the text cannot look
like something else, and it cannot escape the region it is placed in. Both hold regardless of what the model
decides to do with the content.

### 3. The value carries its own isolation, so "isolated" is a property of the data

`IsolatedText` is the only way to obtain a fenced payload, and it holds the body, the rendered form, and what
neutralisation removed. Three accessors rather than one: `body()` returns the neutralised payload alone — for
an inspect or export surface that should not show machine framing — while `render()` and `Display` write the
fenced form. A caller building a prompt cannot get the unfenced form by accident, and a caller showing a user
what was retrieved is not forced to display the markers.

A payload that *mentions* the fence token is altered and **reported** rather than refused. Refusing would make
an ordinary conversation about prompt injection unrecordable, including this platform's own notes; what the
flag buys is that the alteration is visible instead of silent.

### 4. Line structure survives neutralisation

A `\n` and a `\t` are structure, not formatting, and both are control characters. The first implementation
removed every control character, which silently turned a multi-line memory into one run-on line — and a list
rendered as a sentence is a different claim. Both are kept; a carriage return is not, because a lone one is a
line-overwrite primitive and a CRLF is equivalent to the LF beside it.

This was found by a test asserting the exact text, which is the point: the failure mode was a *quiet*
reshaping rather than a loss.

### 5. One message for all retrieved records, after the history and before the question

Each record is fenced individually, and the fences are collected into **one** message that begins with the
introduction. Splitting it into one message per record would interleave untrusted text with the conversation's
own turns, so a record could be read as a turn. The ordering rule from `ADR-0014` — policy, replayed turns
oldest-first, then retrieved records, then the current question — keeps the user's request last, where it is
the thing being asked rather than something buried between retrieved claims.

### 6. Eligibility for a prompt is enforced where the item is built, and the read is a candidate set

The read excludes what only SQL can decide — deleted, proposed, and superseded rows, model inferences, and
task-shaped predicates — and conversion excludes what needs the clock or a policy: a claim that is not current
at this instant, and a type the use case does not allow.

Two of those deserve recording:

- **A model inference is filtered by the read, not by the type check.** `status <> 'proposed'` does not cover
  it: an inference the user *confirmed* is `active` by status and is still the model's own claim. A model's
  previous output re-entering a prompt as evidence is the self-feeding loop the inference boundary exists to
  prevent.
- **`Working` and `Conversation` memories are excluded by the conversion's allow-list.** `Working` is a run's
  own scratch state, and a plan replayed into a prompt reads as an instruction to continue it. `Conversation`
  is dialogue continuity the history replay already provides, so offering it twice would make one turn look
  like independent corroboration of itself.

The two filters are deliberately independent: one is about the *claim*, the other about the *type*, and a
memory can fail either.

### 7. The model may follow an instruction anyway, and that is the design

Prompt injection is **mitigated** here, not solved, and no fencing scheme solves it. What protects the system
is that the *effect* of any instruction still passes through schema validation, authentication,
authorization, risk classification, approval policy, and audit — the model may ask, and deterministic Rust
decides. That is `AGENTS.md`'s boundary rather than a property of this module, and stating it here is what
keeps the fence from being read as a guarantee it is not.

## Consequences

- **The two zero-score states are distinguishable.** A record that neutralised to nothing is refused with a
  reason; a record whose payload was altered is admitted with a count. `looks_like_an_instruction` exists for
  an operator report and is documented as "a `false` is not evidence of safety" — it is wired into no refusal
  path, deliberately.
- **The manifest and the request cannot disagree about retrieved records.** Membership comes from the
  manifest; a record the manifest included but which cannot be found is an **error** rather than a skip,
  because silently omitting it would make the request disagree with its own audit record.
- **Alterations are counted in the run event.** Neutralisation changes what the model reads, so a surprising
  answer has to be attributable to the transform. The count is recorded and the content is not, because a
  memory's text is user content and a run event's retention is not the memory's.
- **The token estimator now measures the rendered payload.** The first version added the two marker lengths
  and forgot the newlines the rendering inserts, so it was two bytes short on every non-empty payload — the
  direction a budget must not err in. It takes the rendering itself now, so there is no length arithmetic left
  to get wrong.

## Honest limits

- **A model may still obey an instruction inside the fence.** See decision 7. This is a mitigation with a
  stated boundary, not a control.
- **Detection is not attempted, and the hint list is not a detector.** `looks_like_an_instruction` fires on
  eight phrases; a rephrasing passes it, and the test asserts that explicitly so a later change cannot
  overstate the guarantee.
- **The format-character range set is duplicated** between `jarvis-core` and `jarvis-mcp`, because
  `jarvis-mcp` depends on `jarvis-core` and not the reverse. Each crate pins the ranges with a test, but a
  change to one list is a change to two files.
- **No ranking, scoring, or diversity is applied to the memories.** `P4-004`'s eligibility and `P4-006`'s
  ranking exist and are tested, and nothing calls them on this path: the read returns the newest rows up to a
  window and the assembler decides what fits. The window is therefore a recency bound rather than a relevance
  bound, and `MAX_MEMORIES_LOADED` is the only thing keeping the candidate set from growing with the user's
  history.
- **`retrieval_count` is never incremented.** `reinforce_memory` exists and nothing calls it, so the
  reinforcement signal is zero for every memory on this path, and `last_accessed_at` stays null. "How often
  has this been useful" is therefore unanswerable from stored data.
- **No vector search.** The semantic signal from `P4-006` is not computed on this path because nothing writes
  an embedding — the same limit `P4-005` and `P4-006` recorded, now visible as "the hybrid ranking is not
  actually hybrid yet".
- **The entity read is not used.** `read_entity_memories` exists and this path reads by workspace only, so
  "what do I know about this person" is still answered by ranking rather than by an index seek.
- **Expiry is evaluated in Rust, not SQL**, so a large table of expired-but-active rows still fills the
  window. A candidate query that could use the index would need the timestamp comparison defect from
  `ADR-0034` fixed first.
- **Nothing records which memories were actually used.** The event says how many were offered, included, and
  altered; it does not say which, beyond the manifest's per-item references not being stored. An operator
  asking "why was *this* used" has a reason for the assembly but no per-memory row.
- **No CLI or route exposes any of it.** `P4-008` owns the inspect and search surface.

## Alternatives considered

- **A pattern-based filter that removes instruction-like text.** Rejected: it is defeated by rephrasing, it
  would corrupt legitimate content that mentions instructions, and it would present a weak heuristic as a
  control. The hint list is kept as a report for exactly that reason and is wired into nothing.
- **Escaping the fence token rather than removing it.** Rejected: an escape needs the reader to understand
  the escape, and removal needs nothing of it — so removal works against a reader that ignores the framing.
- **Refusing a payload that mentions the fence token.** Rejected: it makes an ordinary conversation about
  prompt injection unrecordable, and the alteration is reported instead so it is not silent.
- **One message per retrieved record.** Rejected: it interleaves untrusted text with the conversation's turns,
  so a record can be read as a turn rather than as data.
- **Appending the records to the user's own turn.** Rejected: a record inlined into the user turn is
  indistinguishable from something the user typed, which is precisely the confusion the fence prevents.
- **Trusting the memory's `MemoryTrust` directly as a `ContextTrust`.** Rejected: a model inference would
  become `Derived` and reach a prompt as evidence this platform gathered, and a hedged user statement would be
  labelled as the person speaking.
- **Filtering model inferences by status instead of source kind.** Rejected: a *confirmed* inference is active
  by status, so the filter would admit exactly the case it was meant to exclude.
- **Filtering task-like claims by memory type alone.** Rejected: a user statement *about* a task ("I am working
  on the tax return") is a durable fact that mentions work, and a type filter cannot tell the two apart. The
  predicate filter is narrow on purpose, and the type filter is a second, independent rule.
- **Refusing the run when a memory cannot be converted.** Rejected: a stale or unkeyable memory is normal, and
  making an ordinary expiry fail a question would be a worse outcome than dropping one candidate.
- **Recording the retrieved text in the run event.** Rejected: it duplicates user content into an event stream
  whose retention is not the memory's, and the count plus the manifest is what the audit question needs.
- **Letting the assembler do the type and currency checks.** Rejected: a caller that has a memory and builds an
  item directly would skip them, and the run path is not the only caller `P4-008` will add.
