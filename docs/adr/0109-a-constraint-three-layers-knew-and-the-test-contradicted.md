# ADR-0109: A constraint three layers knew and the test contradicted, and a guard whose only detector was in another binary

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — the Calendar page's two continuation tokens).
- **Relates to:** `ADR-0108` (the same "no type asked the question" shape, one tool over, and the schema
  description that carries a condition), `ADR-0085` (the input/output pairing of one tool), `ADR-0083` (a
  declared output field is bounded by what the request can return), `ADR-0089`/`ADR-0090` (a fixture observed
  rather than requested; a refusal keeps the cursor), and the fixture's own notes, which recorded this
  constraint a slice before the code enforced it.

## Context

The `events.list` reference documents its two continuation tokens as **mutually exclusive**, and it says so in
**each field's own description**:

> `nextPageToken` — "Token used to access the next page of this result. **Omitted if no further results are
> available, in which case `nextSyncToken` is provided.**"
>
> `nextSyncToken` — "Token used at a later point in time to retrieve only the entries that have changed since
> this result was returned. **Omitted if further results are available, in which case `nextPageToken` is
> provided.**"

So a page carries **at most one**, and the two mean opposite things about the walk: a page token continues it, a
sync token ends it and is the position to keep.

**Three layers of this repository already knew that, and none of them made it impossible.** The research record
has a paragraph on it; `calendar_events_list_page.json` carries the rule in its own `_the_point_of_this_fixture`
text, including the sentence *"An earlier test of mine asserted a body carrying both, which Google cannot
produce"*; and the renderer's test helper repeats it. Meanwhile:

1. **`CalendarPage` carried both as independent `Option<String>` fields**, so the impossible pair was a
   perfectly representable value;
2. **`calendar_signal` took `Option<&str>` for the sync token alone**, so a caller could not even say which of
   the two it had read;
3. **the parser's test asserted a body carrying both** — the exact body the fixture's prose said an earlier
   version of *that* test had used and been corrected for. **The correction reached the fixture, the record and
   the renderer's helper, and missed the one test in the parser.**

**And the output schema declared both tokens with no description at all** — bare `["string", "null"]` — one
slice after `ADR-0108` taught the *Gmail* output schema to state its storing condition. A model reading that
schema sees two optional tokens and may persist whichever it saw, including the page token, which **expires when
the walk ends**.

## Decision

**A page carries one continuation, given that the provider allows one.**

```rust
pub enum CalendarContinuation {
    MorePages { page_token: String },
    WalkComplete { sync_token: String },
    NothingFurther,
    Rejected { page_token: String, sync_token: String },
}

impl CalendarContinuation {
    pub fn of_page(next_page_token: Option<String>, next_sync_token: Option<String>) -> Self;
    pub fn storable(&self) -> Option<&str>;
    pub fn page_token(&self) -> Option<&str>;
    pub const fn is_nonconforming(&self) -> bool;
}
```

`CalendarPage` now carries `continuation: CalendarContinuation` instead of two optional fields, the parser
decides the mutual exclusion **where both fields are in hand**, the renderer emits **at most one** token, and
`calendar_signal` takes the type rather than a bare `Option<&str>`.

**`Rejected` is a variant rather than a panic, and rather than a silent precedence.** A conforming provider
cannot send both, so the state names a provider change or a hand-built fixture error. It is a value because a
provider response is external input — a nonconforming one should reach a caller as something it can report, not
as an abort inside a parser. `storable()` and `page_token()` both return `None` for it, so a response nobody can
interpret yields neither a position nor a next page, and `is_nonconforming()` is what makes the state
**actionable** rather than merely representable.

**It is not `HistoryPosition`, and the difference is the shape of the rule.** Gmail's is *"storable only when
the page token is absent"* — a condition on **one** value, which leaves an id present and unstorable. Calendar's
is *"exactly one token"* — a relation between **two** values, which leaves no such state. Sharing one type would
give Calendar a variant its provider cannot produce, which is the defect being fixed. `ADR-0108`'s lesson was
"no type asked the question"; the answer here is not the same type.

**The output schema's descriptions now carry each token's half of the rule**, as the Gmail schema does: the page
token *"continues THIS walk … do NOT store it as a sync position"*, the sync token *"present only on the LAST
page … because it is omitted whenever further results are available"*, and each says it is mutually exclusive
with the other.

**The parser's test keeps the impossible body, deliberately, and asserts it is reported.** Deleting it would
have removed the only exercise of `Rejected`; the assertion is now that the pair is **named, not resolved** —
because a parser that preferred one token would hand a caller a sync position for a walk that has not finished,
which is the defect the type exists for.

## Consequences

- **The constraint is now enforced where it is decidable** rather than recorded in three places that each
  believed the other was checking. The pair reaches a caller as `Rejected`, and neither accessor offers a usable
  value from it.
- **Two guards were falsified A-B-A, both compiling:**
  - `of_page` **preferring the sync token** when both arrive → detected by the parser's test
    (`a pair the provider documents as impossible must be named, not resolved`);
  - routing a **page token through as the sync position** → detected **by the integration test, and by nothing
    in `--lib`**.
- **The second falsification is the finding, and it is about the test suite rather than the code.** The mutant
  survived the entire crate-library suite because `calendar_signal`'s **200 arm had no unit test** — the Gmail
  producer had one and the Calendar producer did not, an asymmetry nobody had reason to look for. A guard whose
  only detector lives in a **different test binary** is one refactor from being unguarded: `cargo test --lib`
  is what a developer runs while iterating, and it was silent.
- **The missing unit test was written**, and re-running the same mutant then fails in `--lib` as well —
  confirmed by mutation rather than assumed. It also closes the asymmetry: both signal producers now have a
  200-path test.
- **`ADR-0108`'s schema-description change was incomplete, and this is the correction.** The Gmail output schema
  was taught to state its condition; the Calendar one was left with undescribed fields, in the same slice that
  added the instruction to state them. A lesson applied to one tool and not its neighbour is indistinguishable
  from a lesson not learned.
- **Limits.** No request is sent and no live walk has run — the path is exercised against the two hand-built
  fixtures. Nothing stores a sync token: there is no sync loop, so `storable` and `page_token` have **no
  production consumer**, and each says so in its doc (the `seconds_from_edge` convention: a stated caller that
  does not exist yet, rather than a value whose absence would leave a page token unreadable while the sync token
  is not). `Rejected` is unreachable against a conforming provider by design, so it is exercised only by the
  test that keeps the impossible body.

## Alternatives considered

- **Delete the parser test that asserted the impossible body.** Rejected: it was the only exercise of the
  nonconforming case, and removing it would have left `Rejected` untested while looking like a cleanup. The body
  is kept and the assertion inverted.
- **Reuse `HistoryPosition` for both tokens.** Rejected: the rules have different shapes (a condition on one
  value versus a relation between two), so sharing would give Calendar a state its provider cannot produce —
  `GmailPosition::UnfinishedWalk`-equivalent carrying a *sync* token would be exactly the impossible pair.
- **Keep two optional fields and add a validation function.** Rejected: a validation function is a rule a caller
  must remember to call, on a value that already looks correct. The pair had three such descriptions and one
  test that contradicted them.
- **Panic on the impossible pair.** Rejected: this is provider input. An abort inside a parser converts a
  reportable provider change into a crash, and the direction this repository keeps choosing is that external
  input yields a value.
- **Prefer the page token when both arrive** (the safer-looking choice, since a page token does not become a
  stored position). Rejected: it silently discards the *other* possibility, and the whole point is that there is
  no way to tell what the provider meant. Reporting is the only answer that does not guess.
- **Collapse `NothingFurther` into `MorePages` with an empty token.** Rejected: an empty token is not an absent
  one (`client::next_page`'s own rule), and "the walk ended with nothing more" is a different fact from "here is
  how to fetch the next page".
- **One `Option<String>` plus a `bool` for which kind.** Rejected: two values that must agree, and the bool's
  `false` would have to mean both "page token" and "no token" — the collapse `ADR-0035` forbids.
- **Leave the schema descriptions for a later slice.** Rejected: the schema is what a **model** reads, and it
  was the layer with the least information and the most authority. `ADR-0108` had already established both the
  problem and the remedy one tool over.

## Conditions that would justify revisiting

- A sync loop walks pages and stores sync tokens, at which point `storable` and `page_token` have their
  production consumers and the "no caller yet" limit is removed.
- A live `events.list` walk of a calendar with more results than one page shows the tokens' behaviour across
  pages, which would confirm the mutual exclusion against a real response rather than against two fixtures.
- A provider response is observed carrying **both** tokens, which would move `Rejected` from "unreachable
  against a conforming provider" to a documented provider behaviour that needs a resolution rule — and the
  direction to resolve it would have to be argued rather than assumed.
- A third tool exposes a pair of mutually exclusive optional fields, at which point "one value, not two
  optionals" is worth stating as a convention in `docs/development` rather than rediscovered per tool.
