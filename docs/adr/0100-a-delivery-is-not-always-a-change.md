# ADR-0100: A delivery is not always a change

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — the Calendar notification-channel push message). Writes the
  research record's own **"Calendar sync-message fixture"** item, which was open.
- **Relates to:** `ADR-0099` (the echoed-channel-token authenticator this makes *readable*), `ADR-0092` (the
  Gmail watch's immediate notification, which is the same "first delivery is not a change" rule, and the "a
  value with no reader" finding), `ADR-0091` (the echoed token is redacted in `Debug`), `ADR-0087` (the Gmail
  watch lease, whose `expiration` this deliberately does **not** share a parser with), and `ADR-0082` (one
  classifier per API — the same "the two providers are different" shape, applied to message shapes).

## Context

`ADR-0099` widened the webhook contract so an **echoed channel token** could be *named* as an authenticator —
Google Calendar's notification-channel mechanism, a client-set string echoed in `X-Goog-Channel-Token`. But an
audit of the push path then found the mirror of `ADR-0092`'s defect **one level up**:

- `crate::google::pubsub` models the **Gmail** push envelope — a `POST` whose body is a `PubsubMessage` whose
  `message.data` is base64 JSON naming a mailbox and a `historyId`.
- **Nothing modelled the Calendar mechanism at all.** A Calendar notification is the *opposite shape*: a
  **zero-length body** and every value in `X-Goog-*` header**s**.

So `ADR-0099` had taught the contract to name a **control whose input the connector could not parse** — an
authenticator declared over a header that no reader looked at. That is `ADR-0092`'s "a value with a producer
and no consumer", inverted: here the *contract* produces the requirement and no code consumes the wire form.
The research record had recorded this honestly for a while — its Verification Plan lists "a Calendar
sync-message fixture" as **not written** — which is the state the slice closes.

The finding that makes the slice worth arguing rather than merely transcribing is the **`sync` message**, and
it is the same rule `ADR-0092` recorded for Gmail from the other direction:

> *"After creating a notification channel to watch a resource, the Google Calendar API sends a `sync` message
> to indicate that notifications are starting… It's safe to ignore the `sync` notification."*

**So the first message on a channel is a handshake, not a resource change.** The Gmail guide says the same in
its own words — a successful `watch` *"immediately sends a notification, so the first delivery is not a
change"* (`ADR-0092`). Both of Google's push mechanisms therefore make "a delivery arrived" and "a resource
changed" differ **on the very first message**, and a consumer that equated them would do one spurious read the
moment it began watching. That is the operational claim this ADR names.

## Decision

1. **A new `google::channel` module models the Calendar notification-channel message**, reading it from
   [`WebhookDelivery`](crate::webhook::WebhookDelivery)'s **headers only**. `parse_channel_message` returns a
   [`ChannelMessage`](crate::google::channel::ChannelMessage) with the always-present fields (`channel_id`,
   `resource_id`, `resource_uri`, `resource_state`, `message_number`) and the two optional ones (`expiration`,
   the echoed channel token). **The body is not read at all**, which is the code-level statement of "there is
   nothing to sign" — and a test feeds a non-empty body to prove the result is unchanged.

2. **The resource state is a closed enum and an unknown value is refused.** `ResourceState` is
   `Sync | Exists | NotExists`, matching the guide's three values **exactly** (case-sensitive), and
   `ChannelMessageError::UnknownResourceState` refuses anything else. The refusal is the deliberate choice
   because **neither default is safe**: treating an unfamiliar state as a change acts on a message the
   connector does not understand, and treating it as a non-change (ignore) could miss a real change. Refusing
   names the problem instead of choosing a direction for the caller.

3. **`is_sync` is the predicate, and detection is by the *state* rather than the message number.** The guide
   states `X-Goog-Message-Number` is *"always 1 for sync messages"* — but that is a fact **about** the sync
   message, not a way to detect one, because the number also *"increase[s] … but [is] not sequential"*. A
   caller keying on `number == 1` would classify any early message as a handshake. So `is_sync` reads the
   **declared discriminator**, and the fixtures deliberately pair a `sync` numbered `1` with an `exists`
   numbered `10` so the number cannot be the thing under test.

4. **`X-Goog-Message-Number` is read as an integer and is not a position or a cursor.** It is surfaced because
   it is a documented header, but nothing treats it as ordering — the same restraint the client module records
   for an opaque `nextSyncToken`. A non-numeric value is refused rather than defaulted.

5. **`X-Goog-Channel-Expiration` is kept as text, deliberately unparsed, because it is the *opposite* encoding
   from the Gmail watch lease.** The Calendar header is *"expressed in human-readable format"* (e.g.
   `Tue, 19 Nov 2013 01:13:52 GMT`), while the Gmail `users.watch` response's `expiration` is an **epoch-millis
   string** ([`crate::google::watch`]). Two expirations, contradictory encodings — so they are read by
   **different code**, and this module does not share a parser with the watch module. A shared `parse` would
   have to guess which encoding a value held, which is the conflation `ADR-0087` removed for the millis case.

6. **The echoed channel token is surfaced and redacted.** `channel_token()` returns it (a verifier needs the
   value), the field is **private**, and the hand-written `Debug` prints `[REDACTED], N chars` in its place,
   because the token is the anti-spoofing control and a value in a log line is a value an attacker could replay
   (`ADR-0091`). Surfacing is deliberately **not** verifying: comparing the echoed value against the stored one
   is a verifier, which this crate does not hold the stored value for, so building one is `P5-010`'s work.

7. **Header reads distinguish absent, ambiguous, and non-UTF-8.** `parse_channel_message` does not use
   `WebhookDelivery::single_header` — which returns `None` for all three — because the remedies differ: an
   absent header is the provider sending less than documented, an ambiguous one (two values) is a wire-level
   attack or a proxy, and a non-UTF-8 one is an encoding fault. Ambiguity is **refused**, so the wrong value
   cannot win.

## Consequences

- **`ADR-0099`'s authenticator now has a reader.** `CHANNEL_TOKEN_HEADER` is asserted **equal** to the header
  an `EchoedChannelToken` scheme names, so a verifier built on the scheme cannot look for a different header
  and find nothing. The "named control, unreadable input" gap is closed at the *read* layer; the *compare*
  layer stays open and is named.
- **The handshake is now impossible to miss.** `is_sync` and the enum make "this delivery is not a change" a
  value the caller must branch on, where before the whole mechanism was unreadable. The finding is the same one
  `ADR-0092` recorded for Gmail, and the two together are the general rule: **a push consumer must distinguish
  "a message arrived" from "a resource changed", and both Google mechanisms make those differ on the first
  message of a watch.**
- **The two expirations are documented as contradictory *on purpose*.** A reader who assumed one `parse` could
  serve both leases now finds the reason it cannot. This is the `ADR-0082` shape — one classifier for two APIs
  answered for the less informative one — applied to a pair of values rather than a status table.
- **Three guards were falsified A-B-A with compiling mutants.** `is_sync` mutated to a constant `false`
  (caught); the unknown-state refusal mutated to `unwrap_or(ResourceState::Exists)` — i.e. guessed as a change,
  the fail-open direction (caught); and the `Debug` redaction mutated to print the token verbatim (caught).
  All three restored byte-identically.
- **⚠ A test-authoring defect was found by a failing test, and it is the "hardcoded arithmetic" family.** The
  redaction test asserted the rendered length as the literal `"34 chars"` (then `"25 chars"`); the token
  `target=myApp-myChannelDest` is **26** characters, so the assertion failed for a mistyped constant and not
  for the behaviour. The fix derives the length from the token (`token.len()`) rather than restating it, so the
  assertion cannot drift from the value it describes. **A literal that duplicates a value the code already
  computes is a second source of truth, and it is wrong exactly when nothing else is.**
- **Two fixtures were added** (`calendar_channel_message.json`, `calendar_channel_sync.json`), each declaring
  `_not_a_capture: true` and the page its shape came from, and both swept by the existing
  `every_fixture_is_json_and_declares_itself` test. The change fixture carries the guide's non-secret example
  token and pins the case-insensitivity of the header lookup by using the guide's `X-Goog-*` spelling, not a
  lowercased copy.
- **A limit remains: nothing receives a Calendar notification, and nothing verifies the token.** This module
  reads a message from headers the crate owns; there is no channel registration, no delivery endpoint, and no
  constant-time comparison. `WebhookSupport` still declares `Polling` for the connector, for the cardinality
  reason `ADR-0099` records.

## Alternatives considered

- **Extend `google::pubsub` to handle both mechanisms.** Rejected: the two are opposite shapes — a JSON body
  whose payload is base64 (Gmail) versus a zero-length body with all content in headers (Calendar) — and one
  module would have to branch on "which mechanism is this" before it could read anything. Two mechanisms, two
  modules, each provable in isolation.
- **Parse `X-Goog-Channel-Expiration` into a `UtcTimestamp`.** Rejected: the value is a human-readable date
  string, and converting it needs a date parser and a timezone policy the crate does not have — while the Gmail
  lease's `expiration` is *already* a numeric string it can convert directly. The two must not share a reader
  because a shared reader would have to guess the encoding; keeping this one as text makes that guess
  impossible.
- **Treat an unknown `X-Goog-Resource-State` as `Exists` (a change).** Rejected — this is the mutant the tests
  kill. It is the fail-open direction: acting on a message whose meaning is unknown. Ignoring it is also wrong
  (a missed change), which is why the honest answer is a refusal that names the value.
- **Detect the `sync` message by `X-Goog-Message-Number == 1`.** Rejected: the number is documented as
  non-sequential, so it is not a stable discriminator, and the guide already gives one — the state. Keying on
  the number would pass the `sync` case and misclassify any other early message.
- **Use `WebhookDelivery::single_header` for every header.** Rejected: it collapses absent, ambiguous, and
  non-UTF-8 into `None`, and this module needs to tell them apart — the same reason
  `SignatureError::AmbiguousHeader` exists beside it.
- **Make `channel_token` a public field.** Rejected: the field is the anti-spoofing control, and
  `ADR-0091`'s rule is that such a value is reached deliberately (through an accessor) and redacted in
  `Debug`, rather than sitting in the struct's public surface where a `{:?}` or a struct update prints it.
- **Compare the echoed token here and report a mismatch.** Rejected: the comparison is a **verifier**, and a
  verifier needs the value the connector registered, which this crate does not hold (`ADR-0099`). Doing it here
  would be a check with no stored value to check against — the "a check nothing supplies" defect this phase
  keeps recording.

## Conditions that would justify revisiting

- **A channel-token verifier is built** (`P5-010`), at which point `channel_token()` gains a comparison and the
  "surfaced, not verified" limit closes.
- **A channel registration flow exists** (watch creation, the `sync`-before-response race, renewal), which is
  where `ChannelMessage` becomes a value the connector *produces* against rather than only reads.
- **`WebhookSupport` gains a way to declare more than one mechanism**, at which point the Calendar push can be
  declared alongside Gmail's and the connector's `Polling` declaration is reconsidered (`ADR-0099`'s condition).
- **The guide documents `not_exists` semantics precisely.** They were **not** established by the page read —
  the guide lists the value but not what it means for a caller — so the choice to treat it as actionable (never
  ignored) rests on the fail-safe direction rather than on a quoted rule. A precise definition would replace
  that reasoning with a fact.
