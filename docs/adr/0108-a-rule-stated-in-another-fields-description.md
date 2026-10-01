# ADR-0108: A rule stated in another field's description, and a cursor the schema told a model to store

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — advancing the Gmail history cursor from a page).
- **Relates to:** `ADR-0085` (a rendered token needs an input that can consume it — the same input/output pairing,
  read in the opposite direction), `ADR-0083` (a declared output field is bounded by what the request can
  return), `ADR-0090` (a refusal keeps the cursor, because the position was never rejected), `ADR-0066` (the
  inference belongs to the caller and the decision to the function), and
  `docs/development/external-research.md`'s rule that a claim is only as good as the page it is quoted from.

## Context

`gmail_history_list` returns two tokens and the connector already knew they were different: a `next_page_token`
continues *this* walk, while the position the page reports is what a *future* incremental sync starts from. The
record says so, the renderer says so, and the type's doc says so.

**The rule is not where any of them looked for it.** The response's `historyId` field is described by the
`users.history.list` reference as nothing more than *"The ID of the mailbox's current history record."* — it says
**nothing** about storing it. The condition that governs storing is stated **once, in a different field's
description** — `startHistoryId`'s:

> "If you receive no `nextPageToken` in the response, there are no updates to retrieve and you can store the
> returned `historyId` for a future request."

So a page's `historyId` is the mailbox's position **at the moment that page was produced**, and a walk with pages
still to come has not consumed the changes up to it.

**And four statements in this crate said otherwise, in four different layers.** Each is *true of a final page and
false of a continuing one*:

| Where | What it said | Why it is wrong |
| --- | --- | --- |
| the renderer's comment | "The history id is the **durable cursor**" | unconditional, and it is only a cursor on the last page |
| `HistoryPage`'s field doc | "The mailbox's **new** position, **which is the next sync cursor**" | same, on the type a caller reads |
| the **output schema's** description | "This is the next sync cursor, and it is NOT the same field as next_page_token." | **a model reads this**, and half of it is wrong |
| `gmail_history_signal`'s doc | "A `200` with no id means **the mailbox was unchanged**" | an inference the response does not support: `historyId` is documented as present on a success, so an absent one establishes no position and is not evidence of no change |

None was load-bearing, which is why all four survived: the parser produced an `Option<String>`, the renderer
emitted it whenever it was `Some`, and the signal producer took `Option<&str>` — so **no type ever asked whether
the id was storable**, and each layer described the field as though the answer were always yes.

**The third row is the one that decides the slice.** An output schema is not documentation for a reader of this
repository; it is an **instruction to a model**, delivered in the model's own channel. A model told "this is the
next sync cursor" stores it, and storing a mid-walk id positions the next sync **past** the changes still sitting
in the pages the walk never read. The loss is silent in both directions: a page token that expired un-walked, and
a cursor asserting that everything before it has been consumed.

## Decision

**A page's stated position is a value qualified by the page token beside it, and the signal producer takes that
value rather than a bare id.**

```rust
pub enum HistoryPosition {
    Storable { history_id: String },
    UnfinishedWalk { stated_history_id: String },
    Unstated,
}

impl HistoryPosition {
    pub fn of_page(stated_history_id: Option<String>, next_page_token: Option<&str>) -> Self;
    pub fn storable(&self) -> Option<&str>;
    pub const fn is_storable(&self) -> bool;
}

pub fn gmail_history_signal(
    status: u16,
    position: &HistoryPosition,
    refusal: RetryDecision,
) -> SyncSignal;
```

**Three variants and not a `bool`.** `UnfinishedWalk` and `Unstated` both fail to yield a position and differ in
*why*, and two of this crate's own rules require the difference to be visible: a stated id that may not be stored
is **information a diagnostic about a stalled walk wants**, and "the provider stated no position" must not be
read as "the mailbox is unchanged" — which is exactly the fourth claim above. Collapsing them would restore that
inference in the type system's own vocabulary.

**The rule is encoded in one place, `of_page`, and it takes both fields.** Neither answers the question alone: an
id with no token is storable, an id with a token is not, and no id is not a position at all — which is why
folding it into an `Option<String>` at the parser (as this code did) loses the distinction whichever way the
`Option` points. A **blank** id is treated as absent rather than as a position, the reading
`VerifiedAccount::new` already takes for a provider identifier.

**The signal takes the type, so an unqualified id cannot be supplied.** This is the change that makes all four
prose claims unnecessary rather than corrected: a caller now cannot reach the signal producer without having
established that the id may be stored. `position.storable()` is named for the question rather than for the value,
and there is deliberately **no** accessor returning the id in every state.

**The output schema's description states the condition and the direction of the mistake**, because that text is
the model's only source for it:

> "The mailbox's current position, and NOT the same field as next_page_token. May be stored as the next sync's
> start only when next_page_token is absent: a page that carries a token has not been walked to the end, and
> storing its history_id skips the changes in the pages that follow."

**The renderer still emits the id on a continuing page.** Withholding it would make *"the page stated an id you
may not use"* look identical to *"the page stated nothing"*, and the condition now travels with the value through
the description plus the presence of `next_page_token` — which is exactly the fact the provider's rule keys on.

## Consequences

- **The rule is falsifiable.** Two guards were falsified A-B-A with compiling mutants, and both were caught:
  - `of_page` returning `Storable` **regardless of the page token** → detected by the rule test *and* the
    parser's page-separation test (`left: Storable`, `right: UnfinishedWalk`);
  - `storable()` returning the id for `UnfinishedWalk` too → detected by the rule test, the parser test **and**
    the end-to-end signal test (`left: Advanced { history_id: Some("12347") }`, `right: … { None }`).
- **`ADR-0090`'s guarantee now has teeth in one more place.** A mid-walk page advances nothing, so
  `advance_gmail_history` keeps the **previous** cursor, asserted against a control: the same id with no page
  token *does* re-anchor. The two cases differ in one fact, so the assertion cannot pass for the wrong reason.
- **A fifth claim was found while writing that control, and it was mine.** The first version used a stated id
  **smaller** than the fixture cursor's, and the test failed — not because of the storable rule but because
  `advance_gmail_history`'s **monotonic** guard refused a backwards move. So the test would have been checking
  the wrong guard. The id is now greater than the fixture's, with the reason recorded in the test, because "a
  fixture that cannot separate two behaviours makes any agreement test pass" applies in the negative too: a
  fixture that triggers a *different* guard makes the test look like it is checking this one.
- **The schema is now part of the safety argument rather than a description of it.** The condition is stated where
  the model reads it, and the value is still rendered, so a model can make the right decision rather than being
  handed a value with no instruction.
- **A limit that was a claim is now a limit that is stated.** A `200` whose page states **no** id — which the
  reference's own schema does not produce, since `historyId` is not marked optional — advances nothing. It is
  `Unstated`, and it is explicitly **not** "the mailbox is unchanged": that would be a conclusion the response
  does not support, and the previous doc asserted it.
- **Unchanged limits.** No request is sent and no live walk has run; the whole path is exercised against the two
  hand-built fixtures. Nothing stores a cursor yet — there is no sync loop — so this is a decision with tests
  rather than enforced behaviour, and the rule reaches a real store only when that loop exists.

## Alternatives considered

- **Correct the four descriptions and leave the `Option` alone.** Rejected: it is what every previous round did,
  and the fifth reader would have had the same four statements to trust. A description cannot be enforced, and
  the defect was not the wording — it was that **no type asked the question**, so no layer had to answer it. The
  schema description is still corrected, but as one of five changes rather than the change.
- **Keep `gmail_history_signal(status, Option<&str>, refusal)` and document the precondition.** Rejected: the
  precondition would be a doc comment on an argument that any caller can violate, and `ADR-0061`'s argument about
  the token applies — a value a caller must *choose* not to misuse is weaker than one the type will not accept.
- **A `storable: bool` beside the id.** Rejected: three states, and `ADR-0035`'s rule. `false` would have to mean
  both "stated but unusable" and "not stated", which are the two the fourth claim collapsed.
- **One `Option<String>` plus a `bool` on the page.** Rejected for the same reason, and worse: two fields that
  must agree, which is the shape `CursorOutcome` exists to avoid by holding a verdict and its value together.
- **Drop the id from a continuing page and render only a storable one.** Rejected: it makes an unfinished walk
  indistinguishable from a page that stated nothing, and it removes information a diagnostic needs. The provider
  states the value; the connector's job is to state its **status**, not to hide it.
- **Make the signal take `Option<HistoryPosition>` so `None` can mean "no position".** Rejected: `Unstated` is
  already that state, and an `Option` around a three-variant enum would give four representations for three
  states — the "two situations, one meaning" defect in the opposite direction.
- **Read a `200` with no id as "nothing changed", as the old doc did.** Rejected: the response establishes no
  position, so the honest reading is that the connector still does not know where the mailbox is. Recording "in
  sync" from the absence of a field is precisely the silent direction this module exists to remove.

## Conditions that would justify revisiting

- A sync loop stores a cursor from this path, at which point the rule moves from representable to enforced and a
  mid-walk page's cursor becomes observable in the store rather than only in the outcome.
- A live `history.list` walk of a mailbox with **more changes than one page** shows the id's behaviour across
  pages, which would confirm the reference's rule against a real response rather than against two fixtures.
- Google marks `historyId` optional in the response schema, which would make `Unstated` a shape the provider
  actually produces — and would make "is an absent id evidence of no change?" a live question rather than a
  hypothetical one.
- A second connector reaches the same *shape* of problem (a value whose usability is established by a **different
  field**), at which point `HistoryPosition`'s pattern — a constructor taking both fields — is worth naming as a
  convention rather than rediscovering per API.
