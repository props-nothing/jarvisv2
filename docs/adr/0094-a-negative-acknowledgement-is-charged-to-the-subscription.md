# ADR-0094: A negative acknowledgement is charged to the subscription

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract — the push handler's response).
- **Relates to:** `ADR-0089` (at-least-once delivery makes a refusal routine), `ADR-0090` (a refusal keeps the
  cursor — the *cursor* half of "what does a failure cost"), `ADR-0092` (the watch response this delivery's
  lease comes from), and `ADR-0093` (a request the provider accepts and ignores — the request half of the push
  path).

## Context

The push path reads a delivery through `pubsub::parse_delivery` and can say whether a status code
**acknowledges** it:

```rust
pub const ACKNOWLEDGING_STATUSES: [u16; 5] = [102, 200, 201, 202, 204];
pub const fn acknowledges_delivery(status: u16) -> bool
```

That is a fact about one response, and it was the whole of the module's acknowledgement story. The push page
publishes a second fact that it does not cover, and that the code had no way to carry:

> "If a push subscriber sends too many negative acknowledgments, Pub/Sub might start delivering messages using
> a push backoff. When Pub/Sub uses a push backoff, it stops delivering messages for a predetermined amount of
> time. This time span can range between 100 milliseconds to 60 seconds."

and in the considerations the same page lists underneath:

> "• Push backoff can't be turned on or off. You also can't modify the values used to calculate the delay.
> • Push backoff triggers on the following actions: When a negative acknowledgment is received. When the
> acknowledgment deadline of a message expires.
> • **Push backoff applies to all the messages in a subscription (global).**"

So the answer to **one** delivery is paid for by **every** delivery on the subscription, for up to a minute, and
the subscriber cannot opt out. A `bool` from `acknowledges_delivery` reads as "true: done, false: this one
retries". The second half of `false` is missing, and it is the half that decides anything: a handler that keeps
refusing a message it will never accept is not retrying a message, it is **slowing every other mailbox on the
subscription** — and doing so indefinitely, because the retry count is bounded by the subscription's policy
rather than by the handler (the page notes a push subscriber "can't modify the acknowledgment deadline of
individual messages").

The concrete failure: a delivery whose payload this connector does not decode — an unwrapped subscription, a
shape that changed — fails identically on every attempt. A `Retry`-forever handler answers a negative code
every time, so the backoff never resets, and every account's notifications on that subscription are delayed for
as long as the bad message survives.

## Decision

1. **`DeliveryAck` is an enum of the three answers, not a `bool` and not the status code.** `Accept` (processed,
   acknowledge), `Retry` (could not process, and a redelivery is the remedy), `AbandonAndAcknowledge` (can never
   be processed, so acknowledge and record the drop). A `u16` would let a caller invent a code and re-derive
   `acknowledges_delivery` at the call site, and would hide the *reason* for the answer behind an integer.

2. **`AbandonAndAcknowledge` is a named variant with its downside written down.** It loses the message from the
   queue, which is a real cost — and it is exactly why the variant exists rather than a default a caller falls
   into. It is the answer whose cost is bounded and local instead of unbounded and shared.

3. **`decide_acknowledgement(retryable, delivery_attempt)` takes the retryability from the caller.** Only the
   caller has the evidence: a held store is transient, a payload this connector does not decode is not, and this
   module sees neither. It is the inference/decision split `ADR-0066` establishes for a cursor, applied to an
   acknowledgement.

4. **The bound is on `deliveryAttempt`, compared verbatim, and `0` means "the provider did not report one".**
   There is no per-message deadline to read, so the provider's own incremented count is the only per-message
   fact available. Absent is not the same as first, and **the direction is chosen**: `0` is below the bound, so
   an unreported attempt still gets a retry — keeping a possibly-new delivery alive, where the opposite reading
   would abandon a first delivery that merely arrived without an optional field. The push page's minimum-value
   example omits `deliveryAttempt`, so the absent case is a shape that occurs rather than a hypothetical.

5. **`MAX_RETRY_ATTEMPTS = 3` is a JARVIS figure and is named as one.** The page publishes the backoff range and
   its global scope but no retry count, so the bound is this platform's policy. It is deliberately small and
   stated, because the alternative — refusing without a bound — is not a policy but the absence of one, and its
   cost is paid by every other mailbox on the subscription.

## Consequences

- **The three answers are asserted to be three, and exactly one refuses.** The test counts the acknowledging
  variants and pins that `Retry` is the only one that costs the subscription, so a change that merged two of
  them or flipped an `acknowledges` arm fails rather than quietly altering what a call site pays.
- **Two guards were falsified A-B-A**, both compiling: disabling the non-retryable arm, and moving the attempt
  bound by one. The second is the off-by-one that is invisible at any distance but the boundary, which is why
  the test walks `0, 1, 2, MAX` rather than asserting two points.
- **A generalisation worth keeping:** *an answer to one message can be a cost to every other.* When a protocol's
  error response feeds a shared control loop, the response stops being a per-message fact — and a predicate that
  returns a `bool` about the message will read as if it were. The question is not "does this code acknowledge"
  but **"who pays for this code"**.
- **And a second, the mirror of `ADR-0093`:** there, an argument the provider **accepts and ignores** was the
  dangerous one because nothing reports it. Here, an answer the provider **honours**, with a cost the sender
  does not see, is the dangerous one for the same reason — the failure is real and untraceable from the call
  site.
- **A limit remains:** nothing sends a response. `DeliveryAck` and `decide_acknowledgement` are the **decision**;
  the HTTP handler that turns them into a status code does not exist, so the mapping from `DeliveryAck` to a
  concrete code is still implicit — `acknowledges()` says *whether* to acknowledge and not *which* code to send.
  That is deliberate (any of the five would do, and choosing one belongs with the handler that owns the response
  object), but it means the round trip from decision to wire is unverified.

## Alternatives considered

- **Keep `acknowledges_delivery` and let a caller choose a code.** Rejected: it is the state that produced this
  ADR — the cost of refusing is not visible at a call site that only knows a `bool`, so the "refuse forever"
  handler is the natural one to write and it is the expensive one.
- **Make `DeliveryAck` carry the status code to send.** Rejected as premature: the five acknowledging codes are
  interchangeable for this purpose, and a decision type that also chose one would put a response object's
  concern into a policy function. `acknowledges()` is the part the policy owns.
- **Bound retries by wall-clock time since the first attempt.** Rejected: there is no first-attempt timestamp in
  the delivery, and inventing one would need state this module does not hold. `deliveryAttempt` is supplied by
  the provider and needs none.
- **Treat an absent `deliveryAttempt` as already exhausted (fail closed).** Rejected: it would abandon a first
  delivery that arrived without an optional field, which is the expensive direction — the message is dropped
  through no fault of its own. A retry is cheap for one message and only becomes a subscription-wide cost after
  the bound.
- **Refuse everything that is not understood, forever.** Rejected: unbounded, and its cost is borne by every
  other account on the subscription. This is the ADR's whole subject.
- **Acknowledge everything, to keep the subscription healthy.** Rejected: it drops deliveries that a single
  retry would have carried, which is the *lost notification* the push path exists to prevent — the same
  "acknowledging everything not obviously wrong" failure `acknowledges_delivery` already refuses. The bound is
  what separates the two: retry while a retry can help, abandon when it cannot.
- **Put `MAX_RETRY_ATTEMPTS` on the subscription rather than in JARVIS.** Rejected: the subscription's retry
  policy is the provider's and the page notes it interacts with push backoff by *adding* to the delay, so this
  bound is about when JARVIS stops *contributing* negative acknowledgements, which is JARVIS's decision.

## Conditions that would justify revisiting

- A push handler is written, at which point the `DeliveryAck` → status-code mapping becomes real and the
  "nothing sends a response" limit is removed.
- Google publishes a recommended maximum retry count for push subscribers, which would turn
  `MAX_RETRY_ATTEMPTS` from a JARVIS figure into a provider one — the distinction this ADR is careful to keep.
- A dead-letter mechanism is configured on the subscription, which would give `AbandonAndAcknowledge` a place to
  *send* the dropped delivery rather than only record it, and would change the balance between the two costs.
- The connector watches more than one mailbox in production, at which point the global-backoff argument is
  measurable rather than documented, and the bound can be set from evidence instead of from the page's range.
