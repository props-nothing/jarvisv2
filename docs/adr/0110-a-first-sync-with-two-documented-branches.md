# ADR-0110: A first sync with two documented branches and one expressible, and an anchor with a reader and no consumer

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — where a just-connected mailbox's first sync begins).
- **Relates to:** `ADR-0092` (a response field with no reader — this is the same gap from the other side),
  `ADR-0097` (two pushes routed on keys of opposite provenance), `ADR-0098` (one address, one account — the
  connect step this extends), `ADR-0023`/`ADR-0087` (a documented figure that nothing applied), and
  `docs/architecture/tools-and-connectors.md` on a first sync being a decision with a cost.

## Context

Three pieces of this connector were built and **nothing joined them**:

- `google::watch::parse_watch_response` reads a `watch` response's `historyId` and describes it as *"the anchor a
  first sync starts from"*;
- `google::request::GmailProfile` carries a `historyId` whose doc says a profile read *"yields the mailbox's
  current position without consuming a message"*, making it *"an alternative way to establish a sync anchor"*, and
  that **either** source *"can seed a first sync"*;
- `google::connection::establish_account` mints the account a first sync runs for, and
  `google::connection::resume_from` decides where that sync begins.

`resume_from` answers `FullSync` for any cursor with no position, and a just-connected account's cursor is
`Start` — so **the only way to begin was to read the whole mailbox**. The anchor had a parser, a field, a
paragraph of justification in two modules, and **no consumer**: reading it changed nothing.

**And the guide documents two branches, not one.** The push guide's *Watch response* section:

> "The response contains the current mailbox `historyId` for the user. **Your client receives notifications for
> all changes after that `historyId`.** If you need to process changes **before** this `historyId`, refer to
> Synchronize clients with Gmail."

with a worked example that makes the first branch concrete:

> "Pass `1234567890` as the `startHistoryId` to `history.list`. Afterward, you can persist `9876543210` as the
> last known `historyId` for future use cases."

where `1234567890` **is** the watch response's `historyId`. So:

- **branch one** — `history.list` from the anchor, which returns the changes since the watch was set (typically
  none) and then yields a position of its own. Nothing is read end to end;
- **branch two** — the mailbox's existing contents, which is what *"changes before this `historyId`"* means.

The connector could express **branch two only**, and its documentation said branch one was the anchor's purpose.
The cost of the gap is not an error: a just-connected mailbox is read in full, which is the single most expensive
operation this connector can perform (`5 + 20N` quota units for *N* messages), to discover that nothing had
happened since the `watch`.

**A second, smaller instance of the same gap.** `VerifiedAccount` and `resume_from` had no way to carry an anchor
at all, so even a caller that *had* the value in hand could not use it: the account type holds an identity and a
grant, and `ResumePoint` had no variant for "we know where to start".

## Decision

**`ResumePoint` gains `FromAnchor`, and the two branches become two functions rather than a flag.**

```rust
pub enum ResumePoint {
    FromPosition { position: String, kind: SyncCursorKind },
    FromAnchor { anchor: String, origin: SyncOrigin },
    FullSync,
}

pub enum SyncOrigin {
    WatchResponse,
    Profile,
}

pub fn resume_from(cursor: &SyncCursor) -> ResumePoint;
pub fn resume_anchored(cursor: &SyncCursor, anchor: &str, origin: SyncOrigin) -> ResumePoint;
pub fn resume_ignoring_anchor(cursor: &SyncCursor, anchor: &str, origin: SyncOrigin) -> ResumePoint;
```

**`position()` returns `None` for `FromAnchor`**, and `anchor()` is a separate accessor, because an anchor is
**not a position**: nothing has been synced from it yet. The two are distinct strings that plausibly fit each
other — which is precisely the confusion the guide's two-number example exists to make visible.

**`requires_full_sync()` is true only for `FullSync`.** An anchored start answers `false`, because it does not
read the mailbox from the beginning. This is the property that keeps the branch a decision: a caller asking "is
this expensive" gets the right answer for both incremental cases without having to know which one it holds.

**A stored position outranks an anchor**, and `resume_anchored` returns `resume_from(cursor)` when the cursor has
a token. The anchor is *historical* once anything has been synced from it, and preferring it would re-read the
window between the watch and the first stored position — a **duplicate** rather than a miss, and a silent one,
since `history.list` would simply return records the store already holds.

**Branch two is its own function, `resume_ignoring_anchor`, and it returns `FullSync` unconditionally** —
including when the account *has* a stored position, because a caller that asked for the mailbox is asking for a
full read and quietly handing back a position would answer a different question. Taking the anchor and
discarding it is the point: the name says which question is asked, so a caller cannot reach the expensive branch
by forgetting an argument.

**Connecting an account keeps the anchor when the caller has one, and the two entry points say which.**

```rust
pub struct AnchoredAccount { pub account: VerifiedAccount, pub anchor: Option<String>, pub origin: SyncOrigin }

pub fn establish_account(...) -> Result<VerifiedAccount, ConnectError>;
pub fn establish_account_with_anchor(...) -> Result<AnchoredAccount, ConnectError>;
pub fn establish_account_from_watch(..., watch: &WatchResponse) -> Result<AnchoredAccount, ConnectError>;
```

`anchor` is an `Option` because a profile or a watch response can state no position, and `None` is the honest
shape rather than a defaulted string: it means the first sync has nothing to start from. The three entry points
share one body, and the origin is carried rather than inferred, so a log line can say **where** a starting point
came from and not only what it is.

**`SyncOrigin` is an enum and not a `bool`.** The two sources are obtainable at different moments for different
costs — a profile read needs only a credential and one quota unit, while a watch also needs a Pub/Sub topic — so a
caller choosing between them is choosing a cost, and `ADR-0035`'s rule applies to a `bool` standing for a choice
between two sources.

## Consequences

- **A just-connected mailbox is no longer read end to end.** The anchor that two modules had already parsed and
  described now has a consumer, and the first sync is `history.list` from it.
- **Both documented branches are expressible, and choosing between them is visible in the type.** Branch two is
  `resume_ignoring_anchor`, which is a *function name* rather than an argument a caller can forget to pass.
- **The guide's two-number trap is now structural.** `position()` and `anchor()` are different accessors, and
  `FromAnchor` carries no `position` — so the reading the guide warns against (*"the anchor a first sync starts
  from"* taken as a position) is not expressible.
- **Two guards were falsified A-B-A, both compiling:**
  - `resume_anchored` **never anchoring** (delegating straight to `resume_from`) → detected
    (`left: FullSync`, `right: FromAnchor { … }`);
  - the position-outranks-anchor check **disabled** → detected
    (`left: FromAnchor { … }`, `right: FromPosition { … }`).
- **A doc comment that was an overstatement is now checkable.** `GmailProfile`'s own doc said *"**either** can
  seed a first sync"* about the profile and watch anchors — true of the *values* and false of the *connector*,
  which had no way to seed anything from either. It is now true of the code as well, and the code is what runs.
- **Limits.** No request is sent; no account is stored; no sync runs, so `ResumePoint::FromAnchor` reaches a real
  `history.list` only when a sync loop exists. The anchor's **freshness** is not modelled: a profile or watch
  response read long ago and used now would anchor at a position that may have been pruned, which arrives as the
  documented `404`/resync path rather than as a local refusal. And nothing records **which** branch a deployment
  chose — a caller picks one per call, so a connector that used branch two for every account would still pay the
  cost it was designed to avoid.

## Alternatives considered

- **Keep `FullSync` as the only start, and treat the anchor as documentation.** Rejected: that is the defect. The
  anchor is a value the provider states, two modules explain at length, and nothing could act on.
- **A `bool` parameter, `resume_from(cursor, anchored)`.** Rejected: the two answers differ by orders of magnitude
  in cost, so the choice should be a **name** at the call site rather than a literal a reader has to look up — and
  a `bool` argument at a call site (`resume_from(cursor, false)`) says nothing about which branch is which.
- **Give `FromAnchor` a `position` accessor that returns the anchor.** Rejected: it would make the accessor that
  exists to distinguish the two values return one for both, which is the conflation the type was added to prevent.
- **Put the anchor on `VerifiedAccount`.** Rejected: an anchor is a *sync* fact and the account type holds an
  identity and a grant (`ADR-0098`); storing a position on the identity would make every identity read carry a
  field only one caller wants, and would make a reconnect silently inherit the old anchor.
- **Let `resume_anchored` fall back to `FullSync` when the cursor is not `Start`.** Rejected: a non-`Start`
  cursor with a token has a position, and `FullSync` would discard it — the "discards a working store" mistake
  `ADR-0090` records, in the opposite direction from the one it names.
- **Make `resume_ignoring_anchor` take no anchor argument.** Rejected: the two functions would then look
  different for no reason a reader can see, which invites calling the wrong one. Taking it and discarding it
  states that the value exists and is deliberately unused.
- **Default a missing anchor to the empty string.** Rejected: an empty `startHistoryId` is a position nobody
  stated, and the direction is the expensive one — `history.list` would be asked for a window from a value that
  names nothing. `None` is what makes "no anchor" reachable.
- **Reuse `SyncCursorKind::MonotonicMarker` as the anchor's kind, so an anchor is an unobserved cursor.**
  Rejected: a cursor that *is not yet stored* and one that *is* are different things, and a `MonotonicMarker` with
  no observed instant would have to invent one.

## Conditions that would justify revisiting

- A sync loop runs, at which point `FromAnchor` reaches a real `history.list` and the branch's cost claim becomes
  measurable rather than reasoned.
- A deployment records which branch it takes per account, which would let the "a connector could still always
  choose the expensive branch" limit be checked rather than assumed.
- The anchor's **age** becomes available at the point of use (a `verified_at` instant beside it), at which point
  a stale anchor could be refused locally rather than surfacing as the provider's `404` — the same direction
  `ADR-0108`'s `HistoryPosition` refuses to guess about.
- A second provider offers a "current position" read that can seed a first sync, at which point `SyncOrigin`'s
  shape (which source, carried rather than inferred) is worth generalising beyond Gmail.
