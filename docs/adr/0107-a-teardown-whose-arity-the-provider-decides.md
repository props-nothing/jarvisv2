# ADR-0107: A teardown step whose arity the provider decides, and a value read for a consumer that did not exist

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — ending the notifications a connection started). Completes the
  channel path `ADR-0100`–`ADR-0106` built at its far end.
- **Relates to:** `ADR-0095` (the teardown order, whose revisit condition this answers), `ADR-0106` (the
  `resourceId` this gives a consumer), `ADR-0093` (the request type this renames and reuses), `ADR-0092` (a
  response field with no reader — the rule this applies to a *field* read and then discarded), `ADR-0103` (a
  composition that forces a value to be carried rather than fetched, here a second time), and
  `ADR-0102` (a channel is identified by what the connector chose, and the identity is a pair).

## Context

The connector could receive a Calendar notification (`ADR-0100`–`ADR-0103`), decide whether a channel's lease
needed replacing (`ADR-0106`), and had a **teardown plan** for ending a Gmail mailbox watch (`ADR-0095`). Three
things were missing at the join, and each was a claim that had already been written down and was not true.

**Finding 1 — `resourceId` was read and discarded one function later.** `ADR-0106` read the field and said, in
the type's own doc, that it is *"what the `channels.stop` call needs"* — and `ChannelRegistration` had **nowhere
to put it**. The `watch` response's `ParseChannelWatchResponse` returns it; `ChannelRegistration::new` took a
channel id, an account and a token; so the value was dropped between the parser and the type that survives to
teardown. That is `ADR-0092`'s defect (a response field with no reader) in a stronger form: **a field with a
reader and no holder**, where the doc states the consumer and the type cannot carry the value to it.

**Finding 2 — a "third step" is not one step.** `teardown.rs` predicted that a future third step (naming
Calendar's `channels.stop`) *"is checked by the same rule instead of by a reader remembering this comment"*, and
`ADR-0095`'s revisit condition asked whether `may_precede` *"generalises or needs a per-API argument"*. Both
assumed the third step would be shaped like the first two. It is not, because the two push mechanisms are ended
by calls of **different arity**, and the guide says so plainly:

> "This method requires that you provide at least the channel's `id` and the `resourceId` properties…
> **Note that if the Google Calendar API has several types of resources that have `watch` methods, there's only
> one `stop` method.**"

With that and the channel's own definition — *"Each notification channel is associated both with a particular
user and a particular resource (or set of resources)"* — the shape follows: `users.stop` ends **the** mailbox
watch (one resource, one call, no arguments beyond the user), while `channels.stop` ends **a** channel and has no
per-user form at all. **An account watching three calendars needs three calls.** So a single `StopWatch` variant
was a claim about the two mechanisms that is false for one of them.

**Finding 3 — and the exposure figure is not shareable either.** `notification_exposure(stop_succeeded)` returns
Gmail's `WATCH_RENEWAL_BOUND_SECONDS` — seven days — when the stop did not happen. That is correct for Gmail
*because Google publishes a bound for the mechanism*. Google publishes **no equivalent bound for a Calendar
channel**: its life is *"determined either by your request or by any Google Calendar API internal limits or
defaults"*. So the only honest figure for a channel is the expiry its own `watch` response reported — and the
function, taking one `bool`, has no way to receive it.

## Decision

**1. A registration carries the identity a channel is stopped by, which is a pair.**

```rust
pub struct ChannelRegistration {
    pub channel_id: String,
    resource_id: String,      // new: the provider's id, kept rather than discarded
    pub account: AccountReference,
    token: Option<SecretValue>,
}
```

`resource_id` is reached through `resource_id()`, and its argument order in `new` follows the stop body's
(`id`, then `resourceId`) so a transposition is visible rather than plausible. It is printed by the hand-written
`Debug`, because it is an opaque identifier for a **collection in the provider's namespace** (`o3hgv1538sdjfh`)
and not a person's value — the same test the token fails.

**2. A Calendar channel stop is its own teardown step, because the provider decides how many calls it takes.**

```rust
pub enum TeardownStep {
    StopWatch,              // users.stop: one call, for the mailbox watch
    StopCalendarChannel,    // channels.stop: one call per channel
    RevokeGrant,
}
```

The plan has three entries and the two stops share `StepPolicy::BestEffort`; the revoke stays `Required`. The
**count is a caller's fact rather than the plan's**: a plan is a sequence of *effects*, so an account with three
channels performs `StopCalendarChannel` three times against three identifiers, and encoding the count here would
make one plan per account and make the shared ordering rule harder to check.

**3. The exposure figure is per-mechanism, expressed by two functions rather than one with a flag.**

```rust
pub const fn gmail_exposure(stop_succeeded: bool) -> NotificationExposure;
pub fn calendar_exposure(lease: ChannelLease, stop_succeeded: bool) -> NotificationExposure;
```

and `NotificationExposure` gains a third state:

```rust
pub enum NotificationExposure {
    SettlingWithinMinutes,
    UntilTheLeaseLapses { seconds: i64 },
    AlreadyEnded { ended_seconds_ago: i64 },
}
```

`AlreadyEnded` is reachable only from `calendar_exposure`, and the asymmetry is real rather than an omission: a
channel has **no** stated bound, so its lease is the only possible input, while Gmail's figure is the documented
constant. Two functions because the inputs differ — a flag would have to be paired with an `Option<ChannelLease>`
that is meaningless in one branch, which is the shape that lets a caller pass a Calendar lease to a Gmail figure.

**4. `WatchRequest` is renamed `JsonRequest`, because its second caller is not a watch.**

It was named for its first and only user. `channels.stop` is the call that *ends* a channel a watch created, so a
name derived from the first user became a claim about the type that is false. The axis that separates this type
from `FormRequest` is the **credential boundary** (`ADR-0093`) — neither body carries one, both authenticate by
the bearer header — so the name follows the axis rather than the caller. `ADR-0093`'s revisit condition asked
precisely whether it should become a `JsonRequest`, and the answer followed the criterion it named.

**5. The builder takes both identifiers, and refuses the empty ones.**

```rust
pub fn calendar_channel_stop(channel_id: &str, watched_resource_id: &str) -> Result<JsonRequest, RequestError>
```

Both go through the **same** `resource_id` validator every other identifier uses, and the refusal names which of
the two is unusable. `renderable_body` stays unredacted: a stop body holds a channel id the connector generated
and a `resourceId` the provider returned, and neither is a secret.

## Consequences

- **`ADR-0095`'s open question is answered: `may_precede` generalises.** The new step was admitted by the rule
  **unchanged and unedited** — two stops before the revoke, no pair refused — because the property the rule tests
  is *authority* (`withdraws_access`, `needs_a_live_grant`), which neither stop carries and revocation does. A
  test now walks every adjacent pair through the rule rather than restating an expected order, so a fourth step
  is a mechanical exercise rather than a re-derivation.
- **`ADR-0106`'s `resourceId` has a consumer, and the consumer is built.** The test asserts the two identifiers
  reach the body in the fields the reference names, which is what makes a transposition fail rather than
  silently stop the wrong channel.
- **A teardown can now be honest per mechanism.** A caller may stop a mailbox watch, three channels, or either
  alone — `may_precede` permits `StopCalendarChannel` before `StopWatch`, because they end different mechanisms
  and neither needs the other.
- **`AlreadyEnded` removes an overstatement.** A channel whose lease had already lapsed exposes nothing, whether
  or not a stop was attempted; a `bool` function reporting `UntilTheLeaseLapses { seconds: 0 }` would have
  described a clean teardown as an open window.
- **Two guards were falsified A-B-A, both compiling:**
  - writing the **channel id into both fields** of the stop body → detected by the parse-and-compare test
    (`left: "channel-alpha"`, `right: "o3hgv1538sdjfh"`);
  - removing the **`stop_succeeded` guard** from `calendar_exposure`'s live arm → detected by the exposure test
    (`left: SettlingWithinMinutes`, `right: UntilTheLeaseLapses { seconds: 90000 }`).
- **A third mutation was a no-op, and the claim it disproved was mine.** Reordering `calendar_exposure`'s arms so
  a successful stop was matched first changed **nothing**: `Lapsed` and `Alive` are different variants, so the
  arms cannot shadow each other and no order decides anything. The doc had said *"that is why the arms are
  ordered lease-first"*, which asserted a fact about the source's **layout** as though it were a fact about the
  **behaviour** — unfalsifiable, and therefore not checkable. The claim was corrected rather than kept: the
  property comes from matching on the variant, and the load-bearing part is the guard on the arm below.
- **Limits.** No request is sent and no teardown executes — there is still no caller, in the same sense that the
  push handler and the sync loop have none. `users.stop` is deliberately **not** built: it is a different method
  with an empty body, so it would need a third request shape for no gain. **The stop permission rule cannot be
  checked**: the guide requires *"the same user from the same client (as identified by the OAuth 2.0 client IDs
  from the auth tokens)"*, and the client id is a claim inside the credential that `crate::google::credential`
  deliberately exposes only as a rendered header value — so a violation surfaces as the provider's `403` rather
  than as a local refusal. And a caller that performs fewer than all of an account's channel stops gets no
  refusal from this module, only a plan to read — unchanged from `ADR-0095`'s note.

## Alternatives considered

- **Add `channels.stop` as one more `StopWatch`.** Rejected: the two are ended by calls of different arity, and a
  teardown that performed one and reported "notifications stopped" is wrong for whichever mechanism it skipped.
  The plan and the exposure figure both have to distinguish them, so the distinction belongs in the type.
- **Put the channel stop in a separate module with its own plan.** Rejected: the ordering constraint is *shared*
  — both stops need the grant the revoke destroys — and two plans would let a caller revoke between them.
- **Give `notification_exposure` a `PushMechanism` parameter so one function serves both.** Rejected, and a
  draft `PushMechanism` enum was written and **removed**: the two figures need different inputs, so the function
  would take a flag and an `Option<ChannelLease>` — and the enum's only remaining use would have been an
  equality assertion in its own test, which is the consumerless-value defect this repository keeps recording.
  Two functions name the mechanism through the call rather than through a value.
- **Keep `WatchRequest`'s name and document that it generalises.** Rejected: a type named for its first caller
  makes every later reader re-derive the axis, and `ADR-0093` already recorded the axis (the credential
  boundary) — the name should state it.
- **Encode the channel count in `PlannedStep`.** Rejected: a plan is a sequence of effects, and the count is a
  fact about the account. It would also make the plan non-constant, since the count is known only at runtime.
- **Let `ChannelRegistration::new` keep three arguments and take the resource id later.** Rejected: a
  registration is the only value that survives between the `watch` and the teardown, so a channel whose
  registration lacks the resource id **cannot be stopped** — the missing argument would be discovered at
  teardown, which is the worst place to discover it.
- **Refuse the echoed `token` in the stop body.** It *is* refused, and the reason is recorded rather than the
  possibility being left open: the reference lists `token` as optional on the stop body, sending it would put
  the anti-spoofing control into a body a diagnostic renders, and the field stops nothing. A test asserts the
  body has exactly the two identifiers.

## Conditions that would justify revisiting

- A teardown executor is written, at which point the ordering and the arity are enforced on a code path rather
  than only representable, and the "nothing calls this" limit is removed.
- The connector learns its **OAuth client id**, which would make the stop permission rule checkable locally
  instead of surfacing as a `403`.
- Gmail's exposure is computed from a **lease** rather than the documented bound, which would let
  `gmail_exposure` report `AlreadyEnded` too — the caller already holds the watch's `expiration`, so this is a
  wiring gap rather than a missing fact.
- A **connector-level** teardown appears (deleting the Cloud Pub/Sub topic or subscription), which is
  `ADR-0095`'s other candidate third step and shares one subscription across accounts — at which point
  `may_precede` gets its second new step, and the "this silences everyone" consequence has to be modelled.
- A third body-bearing request appears, at which point `JsonRequest` is the established shape and the question
  `ADR-0093` left open is settled rather than answered once.
