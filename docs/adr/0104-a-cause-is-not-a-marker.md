# ADR-0104: A cause is not a marker, and the two pushes do not share a state machine

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — composing the Gmail push path, and correcting an overstated
  cross-mechanism claim the composition exposed).
- **Relates to:** `ADR-0103` (the Calendar ingest this mirrors, and whose shared shape it deliberately does not
  reuse), `ADR-0100` (the Calendar handshake, whose marker this contrasts with Gmail's absence of one),
  `ADR-0097` (the router composed here), `ADR-0094` (the acknowledgement charged to the subscription),
  `ADR-0092` (the watch response whose opening notification this is about), and `ADR-0099` (the OIDC-JWT
  authenticator whose verifier is still unbuilt, which is why this ingest has no `Rejected` outcome).

## Context

`ADR-0103` composed the Calendar push path into one `ingest_channel_delivery` and found that the seam is where a
*type* changes. The **Gmail** path had the same shape of gap — `parse_delivery` and `route_delivery` existed and
nothing called them together — so composing it was the obvious next slice. Composing it produced two findings,
and the second is a **correction to work already committed**.

**Finding 1: the outcome spaces differ, and copying the Calendar shape would have invented a state.** The
Calendar ingest has a `Handshake` outcome because Calendar **marks** its opening message: a delivery carries
`X-Goog-Resource-State: sync`, and the guide says *"It's safe to ignore the `sync` notification"* (`ADR-0100`).
Gmail's push guide says the same *kind* of thing about its opening message — *"a successful `watch` call
immediately sends a notification"* — so a reader (me, two rounds ago) naturally generalised to *"so the first
delivery is not a change"*. **Re-fetching the guide to build on that claim shows it is overstated:** the Gmail
notification that `watch` sends carries **no state field**; it is the ordinary `{emailAddress, historyId}`
payload, and it is **byte-for-byte the same shape as a change notification**. There is no `sync`-equivalent for
Gmail. So:

- **The cause is real** — Gmail does send an opening notification when you start watching.
- **The marker is not** — nothing in the delivery says "this is the opening one".
- **Therefore the opening notification is indistinguishable from a change**, and a caller **cannot** skip it the
  way a Calendar `sync` message is skipped. It ingests as a change.

**Finding 2: the overstated claim had already been written into `channel.rs` as a load-bearing comparison.**
`channel.rs`'s module doc said the Calendar handshake is *"the Calendar counterpart of the Gmail rule that a
successful `watch` "immediately sends a notification, so the first delivery is not a change" (`ADR-0092`)"*. That
sentence **attributes to Gmail a detectable handshake it does not have** — it quotes a source but overstates
what the wire carries. It is the `ADR-0074`/`ADR-0096` class again (a doc naming a fact that is not true of the
code), reached from a new direction: here the doc was a **comparison between two mechanisms**, and the
comparison was the thing that was wrong.

## Decision

1. **`ingest_gmail_delivery(body, accounts) -> GmailIngest` composes read then route**, the Gmail counterpart of
   `ADR-0103`'s `ingest_channel_delivery`, in the order the questions must be asked: a body that does not parse
   has no address to route, so reading is first.

2. **`GmailIngest` has three variants and no `Handshake`** — `Unreadable(GmailBodyError)`,
   `Unroutable(DeliveryRoute)`, and `Changed { account, history_id, message_id }`. A `Handshake` variant would
   **claim a distinction the wire does not carry**: with no marker on Gmail's opening notification, a type with
   that state would invite a caller to detect one. **Sharing `ChannelIngest` between the two mechanisms would
   have propagated the error**; the difference is the whole reason for a separate type.

3. **`Unreadable` carries the *layer* that failed** — `GmailBodyError::Envelope` versus
   `GmailBodyError::Payload` — mirroring `parse_delivery`'s own nesting rather than flattening it. A broken
   Pub/Sub envelope is a wire fault; a broken Gmail payload inside a good envelope points at the payload, and a
   single "bad body" message would send a caller to the wrong layer.

4. **`Changed` carries the `message_id` as an `Option`**, because Pub/Sub is **at-least-once**: the same message
   can arrive twice, and `messageId` is the **only** field that distinguishes a redelivery from a new change
   (`ADR-0094`). `None` means *"this may be a repeat I cannot detect"*, which is why it is an `Option` and not a
   defaulted string.

5. **`acknowledgement()` maps `Unreadable` and `Unroutable` to `AbandonAndAcknowledge` and `Changed` to
   `Accept`.** Neither failure is repaired by another attempt — the payload is what it is, the account set is a
   **local** fact — and a negative acknowledgement triggers a **subscription-global** backoff (`ADR-0094`), so
   refusing would slow every other mailbox. A routed delivery is `Accept` because the work is *accepted*; the
   `Retry` variant belongs to a caller that tried to *act* and hit a transient fault, which this function cannot
   see because it decides **what the delivery is**, not whether acting on it succeeded.

6. **There is no `Rejected` outcome, and the reason is a named gap rather than an omission.** Gmail's delivery is
   authenticated by an **OIDC bearer JWT** (`ADR-0099`), and **that verifier is not built** — no JWKS reader, no
   `aud`/`iss`/`exp` check. So there is no control to reject on, and inventing a `Rejected` variant would claim a
   check that does not happen. This is the same "naming an authenticator is not verifying one" limit the
   research record keeps as Unresolved Question 9.

7. **`channel.rs`'s overstatement is corrected in place, and labelled as a correction.** The sentence now says
   what is true: **both** mechanisms send an opening notification, **only Calendar marks it**, and therefore
   Gmail's cannot be skipped. It records that an earlier version claimed otherwise and why the correction
   matters — because a reader who believed the two mechanisms symmetric would reuse one's state machine for the
   other, which is precisely the mistake finding 2 is about.

## Consequences

- **The Gmail push path is now one decision, as the Calendar path is.** Read and route are joined, the outcome
  names the account a sync must use, and a test drives the whole thing from a raw Pub/Sub body string through to
  a routed account.
- **⭐ The general rule, and the one worth carrying forward: a *cause* is not a *marker*.** Google **causes** an
  opening Gmail notification; it **marks** the opening Calendar one. `ADR-0092`'s line — *"the first delivery is
  not a change"* — is true of the *cause* and false of what a **consumer can detect**, and a claim about
  detectability drawn from a claim about causation is how a type gets a state the wire cannot produce. Ask of any
  "X is not a Y" rule: **is that a fact about what happens, or about what the message says happened?**
- **It also restates `ADR-0069`/`ADR-0098`/`ADR-0103`: composition is the oracle.** Finding 1 required **no code
  change to compose** — it fell out of asking *"does Gmail have this state?"* while writing the compose. Finding 2
  was a defect **already shipped** that composing surfaced, because building the Gmail ingest forced the question
  *"which of `ChannelIngest`'s five variants apply here?"*, and the answer was *four do not, and one of the four
  is missing for a reason the existing doc got backwards*.
- **Two guards were falsified A-B-A with compiling mutants.** (a) The routing gate mutated so a non-`Exact` route
  returned a fabricated `Changed` — i.e. acting on a delivery that did not route; caught by the unroutable test.
  (b) The read-error layering mutated so a payload error was reported as an envelope error; caught by the
  layering test with *"a good envelope with a bad payload must report the payload layer, not the envelope"*. Both
  restored byte-identically.
- **The research record is corrected where it compressed the fact.** Its Verification Log already quoted
  *"a successful `watch` immediately sends a notification, so the first delivery is not a change"*; a note now
  records that **the second clause does not follow from the first** for Gmail — the notification is unmarked —
  and the same correction is logged with the re-fetch.
- **A limit remains, and it is the one the whole Gmail push path carries: nothing verifies the delivery.** The
  JWT verifier is unbuilt, so `ingest_gmail_delivery` decides attribution under the **same** trust limitation
  `ADR-0097` records — a forged delivery can select a mailbox to *read* (never a credential to use) and cannot
  cause a **missed** change (the sync runs from the stored cursor). Nothing receives the delivery either: there
  is no endpoint, so this is a decision with tests rather than a running handler.

## Alternatives considered

- **Reuse `ChannelIngest` for both mechanisms.** Rejected — this is finding 2 in type form. `ChannelIngest`
  carries `Handshake` and `Rejected`; Gmail has **neither** (no marker; no verifier). A shared type would force
  one mechanism to have a state it cannot produce or the other to lose one it can, and the doc that motivated
  sharing was itself overstated.
- **Give `GmailIngest` a `Handshake` variant anyway, since `watch` does send an opening notification.** Rejected:
  the *cause* is real but unobservable in the payload, so the variant would be **unreachable by any input** — the
  "a variant nothing constructs" defect this phase keeps finding — and worse, reachable-looking, so a caller
  would branch on it and believe the opening notification was being skipped.
- **Detect the opening notification some other way** (e.g. `historyId` equal to the watch anchor). Rejected: the
  connector does not hold the anchor at ingest time, the comparison would be against a value it would have to
  thread through, and the guide does not promise the opening `historyId` equals the watch response's — inventing
  that would be a second overstated detection rule.
- **Return a `bool` for "acknowledge or not".** Rejected: it erases the difference between a wire fault and an
  unroutable address, which need different diagnostics, and cannot carry the account, position, or dedupe key the
  caller needs.
- **Have `ingest_gmail_delivery` verify the OIDC JWT (build the verifier now).** Rejected for this slice: it needs
  a JWKS client, key rotation, and claim checking, which is `P5-010`'s work and a network dependency in a path
  this crate keeps pure. The honest move is to **omit `Rejected` and name the gap**, not to add a check whose
  inputs do not exist.
- **Carry the full `PubsubNotification` in `Changed` rather than its fields.** Rejected: the caller needs the
  account, the position, and the dedupe key, not the address (the account *is* the address's resolution), and
  passing the raw notification onward would hand the untrusted address to a component with no use for it.

## Conditions that would justify revisiting

- **The OIDC-JWT verifier is built** (`P5-010`, Unresolved Question 9), at which point `GmailIngest` gains a
  `Rejected` outcome and the "unauthenticated delivery" limit closes for this mechanism as it has for Calendar
  (`ADR-0101`).
- **Google adds a state/marker field to its Gmail push payload**, at which point the opening notification becomes
  detectable and `GmailIngest` gains the `Handshake` outcome `ChannelIngest` already has — a change that would
  make the two mechanisms symmetric *for the first time*, and worth an ADR of its own because it reverses
  decision 2.
- **A delivery endpoint / handler is built**, where `ingest_gmail_delivery` becomes *used*, `acknowledgement()`
  acquires a response writer, and the dedupe key acquires a store.
- **The two ingest functions are generalised**, e.g. a shared `DeliveryIngest` parameterised by marker
  availability and verifier presence. Rejected for now (two mechanisms, two types), but a third provider
  (`P5-006` Microsoft) is exactly the point at which the shared shape should be reconsidered — **and the
  parameterisation would have to carry the marker question rather than assume it**, or it repeats finding 2.
