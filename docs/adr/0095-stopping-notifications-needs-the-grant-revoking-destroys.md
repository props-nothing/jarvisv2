# ADR-0095: Stopping notifications needs the grant that revoking destroys

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract — account teardown).
- **Relates to:** `ADR-0087` (the watch lease and its seven-day bound — the figure this ADR's exposure is
  measured in), `ADR-0091` (the mailbox address is redacted because it is a person's), `ADR-0092` (the watch
  response), `ADR-0093`/`ADR-0094` (the request and the acknowledgement that start and sustain the same push
  path), and the revocation module's finding that Google's revocation removes the **project's** grants rather
  than one token's.

## Context

Disconnecting a Google account is **two** operations, and until now this crate had only one of them. The push
path is complete on both sides — the `users.watch` request that starts a lease, the notification decoder, the
acknowledgement decision — and revocation has its parameters, its `200`-means-accepted rule and its effect
table. What is missing is the step that **turns the notifications off**.

`users.stop` is the operation for it, and the reference gives it plainly:

> "Turn off push notification delivery for the given user mailbox. … `POST
> https://gmail.googleapis.com/gmail/v1/users/{userId}/stop`"

Its **authorization** requirement is the same four scopes `users.watch` needs — `mail.google.com/`,
`gmail.modify`, `gmail.readonly`, `gmail.metadata` — so it is an ordinary authenticated API call. And
revocation is the operation this crate already records as **destroying exactly those scopes**:

> "Revocation removes all OAuth 2.0 scopes previously granted to a project, invalidating any issued access or
> refresh tokens for all clients registered under that project."

So the two halves have a **forced order**, and it is the reverse of the one a caller would reach for if it
thought of revocation as "the cleanup at the end":

- **Stop, then revoke** — both succeed. Delivery stops, then access is withdrawn.
- **Revoke, then stop** — the revocation succeeds (RFC 7009 §2.2 makes a `200` cover "the client submitted an
  invalid token", so a dead token is accepted), and the stop that follows is sent with a token the previous
  call invalidated. It fails.

**Why the wrong order costs more than a failed call.** The push guide says of `stop`: *"All new notifications
should stop within a few minutes."* If the stop never happened, that sentence never applies. Nothing ends the
stream but **the lease lapsing**, and nothing renews it, because the grant is gone. The lease's bound is
`WATCH_RENEWAL_BOUND_SECONDS` — **seven days**. So a reversed teardown leaves the mailbox's notifications
arriving at the subscription endpoint for up to a week, where:

- each delivery carries the mailbox address, which is a **person's identity** and precisely the value
  `PubsubNotification`'s hand-written `Debug` redacts (`ADR-0091`);
- the connector **cannot stop it**, because the only call that would have done so needs the credential it has
  just destroyed; and
- the user believes they have disconnected, so nobody is watching for the leak.

That is the finding: **the order of two operations is load-bearing, one of them destroys the other's
authority, and the damage is a silent privacy exposure that no later call can repair.**

## Decision

1. **`TeardownStep` is derived from the two *effects*, not from the two HTTP calls**, and carries the property
   the ordering follows from: `needs_a_live_grant()` is `true` for `StopWatch` and **`false` for
   `RevokeGrant`**. That asymmetry is the trap: a caller that revoked first sees its revocation **succeed** and
   receives no signal that it has just made the next step impossible.

2. **`may_precede(first, second)` is the rule, not a fixed sequence.** It refuses exactly one pairing — a step
   that `withdraws_access()` before one that `needs_a_live_grant()` — and permits everything else including a
   step preceding itself. A function rather than a sentence, so a **third** step (Calendar's `channels.stop`, a
   Cloud Pub/Sub subscription deletion) is checked by the same rule instead of by a reader remembering a
   comment.

3. **`TEARDOWN_PLAN` is the safe order, and a test asserts the rule and the plan agree.** So a plan edited into
   the wrong order fails rather than shipping, and the test states *why* the order is what it is rather than
   pinning a sequence.

4. **The two halves carry different failure policies, and the asymmetry is argued.** `StopWatch` is
   `BestEffort`; `RevokeGrant` is `Required`. A failed stop costs a bounded privacy window and is recoverable
   by a person re-connecting, so it must not abort the teardown — aborting would leave a **working credential**
   in place because a notification preference could not be changed, trading the larger harm for the smaller. A
   failed revoke means the account is **not** disconnected, so the caller is told.

5. **`notification_exposure` is an enum, because only one of its two answers is a figure this crate can state
   honestly.** With the stop accepted, the provider says "within a few minutes" and states **no number** —
   inventing seconds would fabricate a provider rule. Without the stop, the exposure is the lease's bound, which
   Google *does* state, and it is **reused from the watch module** rather than restated so the two cannot drift.

## Consequences

- **Two guards were falsified A-B-A**, both compiling: making `may_precede` return `true` unconditionally, and
  changing the stop's policy to `Required`. The first is the ordering rule itself; the second is the failure
  asymmetry.
- **A generalisation worth keeping:** *when two teardown steps exist, ask which one the other disables.* The
  reversal is invisible at the call site because the **first** call succeeds — the failure lands on the
  *second*, one step later, where a reader is looking at a different operation. This is the same "the report
  and the cost are in different places" shape as `ADR-0094`, in a sequence rather than in one call.
- **And a second, narrower:** *a cleanup you cannot retry after you remove access must happen before you remove
  access.* The window between the two is the exposure, and its length is a provider figure the crate already
  holds.
- **A limit remains:** no request is sent, and **nothing calls this plan** — the teardown executor does not
  exist, in the same sense that the push handler and the sync loop do not. So the ordering is enforced as a
  value with tests and not enforced on any running code path, which is the same "convention with tests, not a
  mechanism" limit `ADR-0091` records. A caller that ignored `TEARDOWN_PLAN` and revoked first would get no
  refusal from this module; it would simply have the plan to read.

## Alternatives considered

- **One `disconnect` operation that does both, hiding the order.** Rejected: the two have different failure
  policies and different authorities, and a single operation would have to pick one policy for both — which is
  exactly the choice this ADR separates. It would also make the exposure figure unreportable, because a caller
  could not say which half failed.
- **Revoke first and let the stop fail, treating the lease as the backstop.** Rejected: that is the defect. The
  backstop is up to **seven days** of a person's address arriving at an endpoint they believe is disconnected,
  and the connector cannot shorten it.
- **Make both steps `Required`.** Rejected: it aborts the teardown when a notification preference cannot be
  changed, leaving a working credential in place — the larger harm chosen to avoid the smaller one.
- **Make both steps `BestEffort`.** Rejected: a failed revoke means the account is still authorised, and a
  caller that was not told would report a disconnect that did not happen.
- **State a number for the "settling" case, e.g. a few minutes as `Some(300)`.** Rejected: the provider gives a
  qualitative statement, and a seconds value beside it would be read as Google's figure while being this
  crate's guess — the distinction `RateLimitEvidence` exists to keep.
- **Reorder by listing the calls in one function rather than in a rule.** Rejected: a future third step would
  then need the ordering re-derived from the sequence, and the property that decides it (`needs_a_live_grant`,
  `withdraws_access`) would be stated nowhere.
- **Delete the Cloud Pub/Sub subscription to end notifications.** Rejected *for now*, not as wrong: the
  subscription is shared by every watched account on the connector, so deleting it to silence one mailbox would
  stop notifications for all of them. It is a candidate third `TeardownStep` for a **connector-level**
  teardown, where the rule in this module would check its pairing.

## Conditions that would justify revisiting

- A teardown executor is written, at which point the ordering is enforced on a code path rather than only
  representable, and the "nothing calls this plan" limit is removed.
- Calendar push is added, bringing `channels.stop` — a third step, and the first real test of whether
  `may_precede` generalises or needs a per-API argument.
- Google publishes a numeric bound for how quickly `stop` takes effect, which would turn
  `NotificationExposure::SettlingWithinMinutes` from a qualitative state into a stated figure.
- A connector-level teardown appears (deleting the topic or subscription), which shares one subscription across
  accounts and would need the "this silences everyone" consequence modelled rather than assumed.
