# ADR-0106: The same expiry in two encodings, and renewal is a replacement

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector contract — the Calendar notification channel's lease). Completes the
  channel path `ADR-0100`–`ADR-0103` built delivery-side.
- **Relates to:** `ADR-0088` (a field Google declares two encodings for — the same shape, one layer over),
  `ADR-0087` (Gmail's watch lease, whose *renewal* this deliberately does **not** share a figure with),
  `ADR-0104` (a cause is not a marker — the two pushes answer to different rules), `ADR-0092` (a response
  field with no reader, which `resourceId` and `token` are on opposite sides of), `ADR-0095` (stopping
  notifications needs the grant that revoking destroys — the consumer `resourceId` is read for), and
  `ADR-0100` (the notification path whose header carries the *other* encoding of this same quantity).

## Context

The channel path could **receive** a Calendar notification and could not **ask for one**. `ADR-0100`–`ADR-0103`
built the delivery side: the header table, the `sync` handshake, the token check, the route, the ingest. What was
missing is everything that happens **around** a channel: the `watch` call that creates one returns a body, and
nothing read it.

Reading it produced two findings, and the first is the one that generalises.

**Finding 1 — the same quantity arrives in two encodings, and the crate could only compare one of them to a
clock.** A channel's expiry exists in two places, and the official pages describe them differently:

- the **notification header** `X-Goog-Channel-Expiration`, which [`ChannelMessage`] already reads, is
  described as *"in human-readable format"* — e.g. `Tue, 19 Nov 2013 01:13:52 GMT`;
- the **`watch` response body's** `expiration`, which nothing read, is described as *"a Unix timestamp (in
  milliseconds)"* — **a JSON number**.

So `ChannelMessage` held the expiry and could not use it: comparing a human-readable date to a wall clock needs
a calendar parser, a locale and a timezone handling this crate does not have. The value that **can** be compared
was the one **not being read**. And the split is not two-way but **three-way** across the two push mechanisms:
Gmail's watch lease (`ADR-0087`) carries its `expiration` as an epoch-millis **string**, Calendar's response
body as an epoch-millis **number**, and Calendar's header as a **human-readable date**. Three encodings of one
quantity, in three documents, for two mechanisms — which is why this slice adds a **third parser** rather than
reaching for Gmail's.

**Finding 2 — Calendar's renewal is not Gmail's renewal, and reusing Gmail's vocabulary would have hidden it.**
The Calendar push guide says:

> "Currently, there's no automatic way to renew a notification channel. When a channel is close to its
> expiration, you must **replace it with a new one** by calling the `watch` method. As always, you must use a
> **unique value for the `id` property of the new channel**. Note that there's likely to be an **'overlap'
> period** of time when the two notification channels for the same resource are active."

That is a **replacement**, not a refresh: the channel is not extended, a *second* channel is created with a
*new unique* id, and both deliver for a while. Gmail's rule is the opposite shape — one watch, renewed against a
7-day bound. `ADR-0087`'s `RenewalAdvice` answers *"how long since this was last renewed"*; a replacement
answers *"when will this channel end"*, which is a question about the **expiry**, not about a cadence. Two
mechanisms, two questions, two inputs.

**A third, smaller finding:** the guide publishes the overlap **qualitatively and gives no number**. So the
margin deciding "close to its expiration" is JARVIS's own figure, and the code must say so rather than imply a
provider rule.

## Decision

**`parse_channel_watch_response` reads the response, and the expiry it returns is the one a clock can be
compared against.** The body's `id`, `resourceId` and `expiration` are read; `expiration` is a **number** of
milliseconds, scaled by one million to the nanoseconds `UtcTimestamp` holds.

```rust
pub struct ChannelWatchResponse {
    pub channel_id: String,
    pub resource_id: String,
    pub expires_at: UtcTimestamp,
}

pub fn parse_channel_watch_response(body: &str) -> Result<ChannelWatchResponse, ChannelWatchError>
```

**A third parser rather than a shared one.** The Gmail decoder and this one read the same *name* in different
*encodings* from different *documents*; a shared parser would have to carry the encoding as a parameter and
could not then state, in one place a reader can check, which document declared which. `ADR-0104` records the
cost of treating the two pushes as one mechanism, and this is the same split one layer down.

**`resourceId` is read because `channels.stop` needs it; `token` is not, because the connector already holds
it.** This is `ADR-0092`'s rule applied twice in one struct with opposite outcomes: `resourceId` has a
**consumer** (the stop call carries exactly `id` and `resourceId`), so reading it is what makes teardown
constructible; the echoed `token` would be a **second source for a value this connector chose**, and `kind` and
`resourceUri` have no consumer at all. Reading a field "because it is there" is how a response type grows
fields nobody can justify.

**The renewal margin is named as JARVIS's own, and is not Gmail's number.**

```rust
pub const CHANNEL_REPLACE_LEAD_SECONDS: i64 = 24 * 60 * 60;

pub enum ChannelRenewal {
    ReplaceNow { lapsed_for_seconds: i64 },
    ReplaceSoon { remaining_seconds: i64 },
    NotYet { remaining_seconds: i64 },
}
```

It happens to equal `WATCH_RENEWAL_RECOMMENDED_SECONDS`, and it is **stated separately anyway**: reusing the
Gmail figure would tie two independently documented mechanisms together, so a change to one provider's text
would silently move the other's behaviour. Same value, two constants, so each can move with its own evidence.

**Three variants and not two.** A lapsed channel and a nearly-lapsed one call for the same *action* and carry
different *operator meaning*: one has already stopped delivering — notifications are being **lost** — and the
other must be replaced **before** it does. Collapsing them would hide whether a gap has already begun, which is
the same distinction `RenewalAdvice` draws for Gmail.

**The boundary instant counts as lapsed**, in nanoseconds, for the same two reasons `watch_lapse` decides
Gmail's lease that way: truncating to seconds would call a channel with half a second left either answer, and
treating the boundary as alive keeps a dead channel — and its silent notification loss — one interval longer.

**Four distinct errors**, because the remedies differ and each points a reader at a different layer: `NotJson`
(the body), `Missing { field }` (which field to add or look for), `WrongType { field, expected }` (the
**encoding** — a *string* here is one of the other two encodings, and a caller must not be told the field is
absent when it is present in the wrong form), and `OutOfRange` (the unit or the value, where an absurd
millisecond count would otherwise wrap).

## Consequences

- The connector can now read a `watch` response, so the channel it created has a **known expiry** and a
  comparison against a clock; before this, a caller had an opaque channel and a human-readable header.
- **Renewal is expressible as what the provider says it is.** Three states, on the **expiry** input the
  provider actually gives, rather than a cadence borrowed from the other mechanism.
- **The triple-encoding of one quantity is recorded in code, in three parsers, with the documents named.** A
  reader who wants to know which encoding to trust for which mechanism has one place to look.
- `ChannelMessage`'s module-level claim that the header expiry is unreadable-as-an-instant is now **load-bearing
  rather than incidental**: the response body is what made the comparison possible, and the doc says which
  document declares which encoding.
- **Two mutants were falsified A-B-A, both compiling:**
  - `checked_mul(1_000_000)` → `checked_mul(1_000)`, i.e. reading milliseconds as microseconds (detected by
    the test that pins the guide's own `1426325213000` to `1426325213` seconds);
  - the renewal boundary `for_seconds <= CHANNEL_REPLACE_LEAD_SECONDS` → `<` (detected by the test that
    asserts a channel exactly one lead away is `ReplaceSoon`).
- **The scale factor is pinned against the guide's own example value**, because a wrong unit is the one error
  here that does not announce itself: `1426325213000` read as micro- or nanoseconds yields a valid `UtcTimestamp`
  in the wrong century, and no type can catch a plausible unit.
- **Limits.** No `watch` call is issued and no live channel has been read — a hand-built fixture stands in
  (`calendar_channel_watch_response.json`, marked `_not_a_capture`). `expiration` may come back **shorter than
  requested** ("determined either by your request or by any Google Calendar API internal limits or defaults
  (the more restrictive value is used)"), which the type doc records and no test can exercise offline. The
  replacement itself — the new unique `id`, the second `watch`, the overlap and any deduplication of the
  deliveries that overlap produces — is **not built**: this slice decides *when*, and the caller that *acts*
  is still missing, exactly as `ADR-0087`'s Gmail advice has no scheduler.

## Alternatives considered

- **Read the expiry from the header `ChannelMessage` already parses.** Rejected: the header is a
  human-readable date, so a comparison needs a date parser, a locale and a timezone — machinery this crate
  deliberately does not carry (`ADR-0088`'s "prefer the reading whose error repairs itself"). The response body
  is the same quantity in an arithmetic form, so using it costs nothing and removes a parser instead of adding
  one.
- **Share one expiry parser between Gmail's lease and Calendar's response.** Rejected: the two encodings differ
  (string vs number) **and** the documents differ. A shared parser would need the encoding passed in, and the
  call sites would then be the only place the provider's own words are recorded — which is how a documented
  contradiction silently becomes a parameter default.
- **Treat a JSON string `expiration` as acceptable here, since another mechanism sends one.** Rejected: the
  response reference says *number*. Accepting a string would paper over a real shape change at the provider,
  and the `WrongType` variant refusing it is what would report that change instead of accepting it silently.
- **Reuse `WATCH_RENEWAL_RECOMMENDED_SECONDS` / `RenewalAdvice` for Calendar.** Rejected: `RenewalAdvice` takes
  a *last-renewed* instant because Gmail's rule is a cadence; Calendar has no cadence to measure, only an
  expiry, so the function would have to invent a start instant. `ADR-0104` records what conflating the two
  mechanisms cost the last time.
- **Two variants — "replace" vs "do not".** Rejected: it cannot say whether notifications are already being
  lost, which is the difference between a scheduled replacement and an incident.
- **Report the overlap as a documented number.** Rejected: **there is no number.** Inventing one and citing the
  guide would attribute a figure to a document that does not contain it — `ADR-0080`'s defect.
- **Read `kind`, `resourceUri` and the echoed `token` because the response carries them.** Rejected: no
  consumer for the first two, and a second, weaker source for the third. `ADR-0092`.

## Conditions that would justify revisiting

- A live `watch` call shows the response's `expiration` in a different encoding (or shape) from the one the
  reference declares, which would turn this parser's `WrongType` refusal into a real, reported contradiction
  rather than a hypothetical one.
- Google publishes a **figure** for the overlap window or a recommended replacement lead, at which point
  `CHANNEL_REPLACE_LEAD_SECONDS` stops being a JARVIS choice and becomes a provider figure that should be
  recorded as such.
- `channels.stop` is implemented, which is the consumer `resource_id` is read for; if the stop call turns out to
  need nothing else from this response, the "read it because nothing acts on it" rule above keeps holding.
- A scheduler issues the replacement, at which point the overlap becomes observable: two channels delivering
  for one resource is exactly the duplicate-delivery case the at-least-once path must deduplicate, and the
  deduplication key would be worth deciding then rather than now.
- Calendar gains a way to **extend** a channel, which would make `ChannelRenewal`'s "replace" vocabulary wrong
  rather than merely specific.
