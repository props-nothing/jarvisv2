# ADR-0103: Composing the push path is what decides, and the seam changed a type

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — composing read, verify and route into one ingest decision).
  Completes the Calendar push path `ADR-0100`, `ADR-0101` and `ADR-0102` each built one piece of.
- **Relates to:** `ADR-0069` (two tested halves do not test the seam between them), `ADR-0098` (the same
  "the join was the missing step" finding for the Gmail connect flow), `ADR-0094` (a negative acknowledgement
  is charged to the subscription), `ADR-0100` (the handshake is not a change), `ADR-0101` (the token verifier),
  `ADR-0102` (the router this feeds), and `ADR-0100`'s `ChannelRoute`.

## Context

Four slices built the Calendar push path one piece at a time, and **nothing called them together**:

- `parse_channel_message` (`ADR-0100`) reads a delivery from its `X-Goog-*` headers.
- `verify_channel_token` (`ADR-0101`) compares the echoed token in constant time.
- `route_channel` (`ADR-0102`) attributes the delivery to an account by its channel id.
- `is_sync` (`ADR-0100`) distinguishes the handshake from a change.

Each was correct alone and each was proven alone. That is exactly the state `ADR-0069` warns about — *"two
tested halves do not test the seam between them"* — and the **same** shape `ADR-0098` found for the Gmail
connect flow: "every piece of the push path existed and the join did not." Composing them is not bookkeeping; it
is where the questions a caller actually has get asked for the first time.

Composing them **found two things the pieces could not**, which is the finding:

1. **No single value could express the real outcomes.** A caller would have to route (getting an account) and
   then *look the registration up again* to get the token — **two lookups deciding one match**. And the result
   still could not be one value meaning *"the channel verified and this delivery is the handshake"*: a
   `Result<Option<AccountReference>, _>` would encode "handshake" and "not routable" as the same `None`, two
   states that need **opposite** handling.
2. **The type that carried the account could not carry the control that proved it.** `ChannelRoute::Exact`
   carried only an `AccountReference`, but verification needs the **stored token**, which lives on the
   registration. So `Exact` had to change to carry the whole `ChannelRegistration` — and that change is what
   makes route → verify a **single** match rather than two scans that merely happen to agree.

## Decision

1. **`ingest_channel_delivery(delivery, registrations) -> ChannelIngest` composes the four pieces in the order
   the questions must be asked**: read, then route, then verify, then classify. Each step is first for a reason:
   an unreadable delivery has nothing to route; routing must precede verification because the stored token
   belongs to a registration; and the handshake/change classification is read **last**, after the delivery has
   proved it is for this channel — so an unauthenticated delivery cannot steer whether work happens.

2. **`ChannelIngest` has five variants, and only one means "act".**

   | variant | well-formed? | verified? | a change? | what to do |
   | --- | --- | --- | --- | --- |
   | `Unreadable` | no | — | — | record the drop |
   | `Unroutable` | yes | — (not checkable) | — | record the drop |
   | `Rejected` | yes | **no** | — | record the drop |
   | `Handshake` | yes | yes | **no** | accept, act on nothing |
   | `Changed` | yes | yes | **yes** | **accept and sync** |

   It is not a `Result`/`Option` because those collapse distinctions that need opposite handling: `Ok(None)`
   would merge *handshake* (accept, do nothing) with *unroutable* (a stray or forged delivery), and `Result`
   cannot separate a refusal from a retry. `Unreadable`, `Unroutable` and `Rejected` carry their specifics —
   the parse error, the route, the token check — because a *diagnostic* must name which layer stopped it.

3. **`ChannelRoute::Exact` carries the whole `ChannelRegistration`, not only its `AccountReference`.**
   **This is the type change the composition forced, and it is the load-bearing decision.** The token that
   *proves* the delivery lives on the registration, so a route that kept only the account would force the verifier
   to re-scan and re-decide the match — and then the account acted on and the token verified could come from two
   lookups that merely happened to agree. Carrying the registration makes them **one** lookup. A
   `registration()` accessor returns it; `account()` still returns just the reference.

4. **`Unroutable` is never `Rejected`.** Routing runs before verification, so a channel that is not registered
   **cannot be verified at all** — there is no stored value to compare against. Reporting it as `Rejected` would
   imply a comparison happened. The two are separated by the same rule `ADR-0101` uses to keep `Absent` from
   `Mismatch`: *was a control present to fail?*

5. **An un-tokened channel still syncs on a real change.** `ChannelTokenCheck::Absent` (no token registered)
   **may be acted on**, so a correctly configured, un-tokened channel is not silently dead — the guide makes the
   token optional. But its `sync` message is still a `Handshake`, so accepting a change does not accept
   everything.

6. **Every outcome acknowledges.** None of the four non-`Changed` results is repaired by another attempt — a
   malformed body, an unregistered channel, a failed token and a handshake all fail or repeat identically — and
   a negative acknowledgement triggers a **subscription-global** backoff (`ADR-0094`), so refusing would slow
   every other channel for a message that can never become actionable. `ChannelIngest::acknowledges()` is a
   method rather than an omitted fact so the intent is read, and so a future variant that *should* be retried has
   a place to say `false`.

7. **`Changed` carries only the `AccountReference`, not the registration.** A sync needs the account; handing the
   token onward would put the channel's anti-spoofing control into a component with no use for it. The token is
   used for verification and then **left behind**.

## Consequences

- **The Calendar push path is now one decision, tested end to end against the recorded wire shapes.** A harness
  test drives `ingest_channel_delivery` with the two channel fixtures and asserts all five outcomes as
  circumstances change (matching token, no registrations, wrong token, no token registered) — so the seam is
  exercised, not just the pieces.
- **⚠ The finding is that the seam is where a *type* had to change, and no per-piece test could have shown it.**
  Read/verify/route were each green; the defect was that `Exact` carried too little to compose them without a
  second lookup. **The general rule: when a composition forces a field to be *fetched* rather than *carried*, the
  missing field is the finding** — here, the token that proves what the account is allowed to act on.
- **`ADR-0069`'s "two tested halves" and `ADR-0098`'s "the join was the missing step" are the same defect, now
  seen a third time.** The reusable statement: **a slice that builds a function and a slice that builds its
  input are not a slice that shows the two working together** — and the join is where a type's *insufficiency*
  becomes visible, because composition is the first caller that has to pass a value from one to the other.
- **Three guards were falsified A-B-A with compiling mutants.** (a) The verification step disabled
  (`if false && !check.may_be_acted_on()`) — a mismatched token became `Changed` and would have synced, caught by
  two tests. (b) The handshake classification disabled (`if false && message.is_sync()`) — a handshake became
  `Changed`, caught by the handshake test. (c) **The token verified against a *different* registration**
  (`registrations.first()` instead of the matched one) — a delivery for channel B carrying B's token was checked
  against A's, caught by the one-registration test. All three restored byte-identically.
- **A limit remains: nothing receives the delivery.** There is still no endpoint and no channel store, so
  `ingest_channel_delivery` is a **decision with tests** rather than a running handler — the "convention with
  tests, not a mechanism" limit every slice of this mechanism carries. It does not record the drop, sync
  anything, or deduplicate a redelivery (`X-Goog-Message-Number` is available but unused, per `ADR-0100`'s
  "not a position"). And the Gmail `OidcIdToken` verifier is still unbuilt (Unresolved Question 9), so the Gmail
  path has no equivalent composition yet.

## Alternatives considered

- **Return `Result<Option<AccountReference>, IngestError>`.** Rejected: `Ok(None)` would have to mean both
  *handshake* (accept, do nothing) and *not routable* (drop), and those need opposite handling — the exact
  collapse `ChannelRoute` was built to avoid. A `Result` also cannot separate a refusal from a retry.
- **Have `ingest_channel_delivery` return the `ChannelRegistration` for a `Changed` outcome.** Rejected: the
  sync needs the account, and passing the registration onward would carry the channel token into a component
  with no use for it — the narrower value is the safer one, and the verifier is the only place that needs the
  token.
- **Keep `ChannelRoute::Exact(AccountReference)` and look the registration up again.** Rejected — this is the
  mutant the one-registration test kills, and it is the whole finding: two lookups for one match let the
  account acted on and the token verified come from different scans. Carrying the registration removes the
  second lookup rather than trusting it to agree.
- **Report an unregistered channel as `Rejected`.** Rejected: verification needs a stored token, and an
  unregistered channel has none, so no comparison happened. `Rejected` would claim a control failed when none
  was present — the `ADR-0101` "missing control ≠ failed control" rule, applied at the composition.
- **Return `bool`/status instead of the five-variant enum.** Rejected: the outcomes differ in *what a caller
  does* (record / accept-and-do-nothing / sync) and in *who is at fault* (this connector's registration
  collision vs a stray or forged delivery), which a boolean or a status code cannot carry.
- **Treat `Absent` (un-tokened channel) as a rejection at the composition.** Rejected: the token is optional, so
  this would make every correctly configured un-tokened channel **never sync** — failing closed on a documented
  configuration, the same mistake `ADR-0101` rejects.
- **Have the ingest function also acknowledge/record.** Rejected: it has no store and no response, and mixing a
  decision with its side effects would make the decision untestable without a transport — the boundary every
  function in this mechanism keeps.

## Conditions that would justify revisiting

- **A delivery endpoint / handler is built**, which is where `ingest_channel_delivery` becomes *used* and where
  `acknowledges()` acquires a writer and the drop acquires a sink.
- **Redelivery dedupe is added.** `X-Goog-Message-Number` is read but unused (`ADR-0100` records why it is not a
  position); a dedupe key would be a new ingest outcome or a pre-step, with its own reasoning about
  at-least-once delivery.
- **The Gmail path gains a composition.** The `OidcIdToken` verifier (Unresolved Question 9) must exist first,
  and then `route_delivery` + `verify` + the handshake rule would want the same single-`ChannelIngest`-shaped
  value — which is where the two mechanisms' compositions either share a type or are shown to differ.
- **`ChannelRoute::Exact` carrying the registration proves unnecessary** (e.g. a store yields the registration by
  id in one call), at which point the reference would suffice and decision 3 could be revisited — but only with
  a caller that makes the second lookup impossible rather than merely redundant.
