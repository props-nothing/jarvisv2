# ADR-0089: A rule from another context, and the observable axis

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0088` (the encoding contradiction this corrects), `ADR-0086` (a rule applied to the
  wrong subject), `ADR-0082` (an argument from one API applied to another), `ADR-0079` (a figure read as the
  wrong thing).

## Context

`ADR-0088` decoded the Pub/Sub notification payload under both alphabets and **refused `=` padding outright**,
on this reasoning:

> Padding is refused, not tolerated. Neither form this crate accepts is padded (URL-safe because RFC 7636
> requires padding omitted; standard because the caller passes what its encoder produced).

Both halves of that are **true and irrelevant**. RFC 7636 is an **OAuth PKCE** rule about the code verifier;
`message.data` is a **Cloud Pub/Sub** field. And the "caller passes what its encoder produced" was a statement
about *this crate's* callers, not about the provider.

Then the provider's own documentation contradicted it. The Pub/Sub push page's minimum-value example of
`message.data` is:

```
SGVsbG8gQ2xvdWQgUHViL1N1YiEgSGVyZSBpcyBteSBtZXNzYWdlIQ==
```

which ends in `==`, decodes to `Hello Cloud Pub/Sub! Here is my message!`, and is therefore **padded standard**
base64. The connector would have **refused the provider's own published example** — a delivery it does not
understand, which on a notification path is a **missed change**.

**A rule was applied from a context that does not govern the value it was applied to.** That is the same
conflation `ADR-0082` records (Gmail's error vocabulary applied to Calendar) and `ADR-0086` records (a
validator reporting a field name it was not given), one level up: here the *justification* was imported rather
than the vocabulary.

A second problem surfaced while fixing it. Attempting the four combinations (two alphabets × two paddings)
produced a test failure that could not be repaired: a value whose unpadded length is already a multiple of four
is **valid base64 either way**, so a padded attempt cannot be distinguished from an unpadded one, and the
"padded" attempt can fail for a value that is legal. Trying both paddings was **searching for something
observable**.

## Decision

**Padding is observed, not requested; only the alphabet is an argument.**

1. **`decode_with(input, alphabet)` takes one axis.** The alphabet is a parameter because it is
   **unobservable**: the two alphabets differ in exactly two characters, so a value containing neither decodes
   identically either way and no inspection can recover which was intended. Padding is **observable** — it is
   the `=` at the end — so the decoder reads it, and a caller is never asked for a fact the value already
   states.

2. **`Padding` is `Absent | Present`, a reported property.** It is still worth reporting, because it is the
   axis separating the two published forms of the same field: the Gmail guide's example is unpadded and the
   Pub/Sub page's own is padded. `Padding::of(value)` is a lookup, and its doc says the only way to be wrong is
   to misread the string.

3. **Padding is validated, not merely stripped.** At most two `=` and only after a whole group; anything else
   is `Base64Error::Padding`. It is checked **before** the character loop so a `=` is not reported as an
   alphabet fault — naming the wrong problem, which is `ADR-0086`'s defect in a different place.

4. **`PubsubData` keeps two axes, and the two documented forms are associated constants rather than variants.**
   `GMAIL_GUIDE` is `{UrlSafe, Absent}` and `PUBSUB_FIELD_TYPE` is `{Standard, Present}`. A four-variant enum
   would have made the documents into cases; they are points in one space, and a document changing one axis
   should not need a new variant.

5. **The decode order is the two alphabets, URL-safe first** — the Gmail guide is the more specific statement
   about *this* payload, while the `PubsubMessage` reference describes a general field Gmail reuses. Padding no
   longer participates in the search, because it never needed to.

**And the envelope is parsed, which the previous slice left open.** `PubsubDelivery` carries `subscription`,
`deliveryAttempt`, and the `message` object with `data`, `messageId` and `publishTime`:

6. **`messageId` is the deduplication key, and it is necessary because delivery is at-least-once.** The push
   page: *"A non-success response indicates that Pub/Sub must resend the messages"* and *"If you send a
   negative acknowledgment or the acknowledgment deadline expires, Pub/Sub resends the message."* A repeat is
   therefore normal, and without the identifier a repeated Gmail notification is processed twice.

7. **Both spellings of the metadata fields are accepted.** The push page's own examples show `messageId`
   **and** `message_id`, and `publishTime` **and** `publish_time`. A parser reading only one finds `None` for
   the other and reports **no error**, so the deduplication key would be silently absent exactly when it is
   needed.

8. **`ACKNOWLEDGING_STATUSES` is the page's list of five, not "2xx".** `102`, `200`, `201`, `202`, `204`
   acknowledge; anything else is a negative acknowledgement and Pub/Sub resends. A `203` or `206` is a success
   by HTTP's classification and a **negative acknowledgement** by this one, so a handler returning
   two-hundred-and-something would silently request redelivery of everything. `acknowledges_delivery` **fails
   closed** — an unrecognised code is treated as a negative acknowledgement, which causes a redelivery rather
   than a lost notification.

## Consequences

- The connector decodes the provider's own published example, which the previous slice would have refused.
- **The rule and the value now live in the same context.** The alphabet is asked for because it cannot be
  seen; the padding is read because it can. A future provider that pads differently needs no decoder change.
- **The envelope's fields are in the places the provider puts them.** An earlier attempt had `messageId` and
  `publishTime` at the top level, where every lookup returned `None` — silently, because the fields are
  `#[serde(default)]`. The test caught it by asserting the field is `Some` for the provider's own example
  rather than merely that the body parses.
- **Three mutants were falsified A-B-A**, all compiling:
  - dropping the standard alphabet (detected by the two-axis test **and**, after the regression test was
    strengthened, by the padded-example test);
  - making `Padding::of` always report `Absent` (detected by both);
  - and the first mutant attempt **survived**, which is the useful part: the padded-example test passed under it
    because `PUBSUB_PAGE_EXAMPLE` uses only `A-Za-z0-9` and therefore decodes under *either* alphabet. The
    regression test now forces the alphabet too, so a mutant cannot hide behind the example's convenience.
- **A dead-code smell was removed rather than papered over.** `decode_with` computed the padding and discarded
  it — the first `Padding::of` mutant changed nothing because it mutated that dead computation. The padding is
  now discarded **deliberately and with a comment**, and the one place it is read is `Padding::of`, which is
  where a caller and a mutant both look.
- **A limit remains:** no delivery has been received, so which alphabet and padding a live subscription uses is
  still **unknown** — that is what reporting `PubsubData` leaves open. `attributes` and `orderingKey` are not
  modelled: nothing in the Gmail push path filters on an attribute, and order is opt-in and unclaimed here. And
  `publish_time` is carried as text rather than parsed, because nothing reads it yet and a timestamp type would
  invite ordering logic the provider explicitly does not guarantee.

## Alternatives considered

- **Keep trying both paddings and accept whichever works.** Rejected: it is a search for something observable,
  and it **cannot work** — a value whose unpadded length is already a multiple of four is valid either way, so
  the padded attempt is indistinguishable from the unpadded one and can fail on a legal value. The test failure
  that exposed this is recorded in the consequences.
- **Keep refusing padding, since the Gmail guide's example is unpadded.** Rejected: the Pub/Sub page's own
  example is padded, and refusing the provider's documentation is refusing a delivery.
- **Accept padding but do not report it.** Rejected: it is the axis separating the two published forms, so
  reporting it is what lets a real delivery settle which one is used — the same argument that keeps
  `PubsubData::alphabet` visible.
- **Model `messageId`/`publishTime` at the top level, as the first attempt did.** Rejected by the test: the
  provider puts them inside `message`, and `#[serde(default)]` makes the resulting `None` silent.
- **Treat any `2xx` as an acknowledgement.** Rejected: the page lists five codes, and `203`/`206` are successes
  that are *not* on it. A handler would then negative-acknowledge every delivery with no error anywhere.
- **Read only the camelCase spellings.** Rejected: the push page shows both, and a `#[serde(default)]` field
  that matches neither is `None` rather than an error.
- **Delete `ADR-0088` and rewrite it.** Rejected: the superseded reasoning is the finding. An ADR that is
  quietly corrected teaches nothing; one that records *why* the correction was needed is what stops the next
  imported rule.

## Conditions that would justify revisiting

- A real delivery logs a `PubsubData`, which would settle the alphabet and padding from evidence and let the
  second alphabet become a deliberate compatibility affordance.
- A slice needs `attributes` or `orderingKey` — deduplication by attribute, or ordered processing — at which
  point the envelope grows a field whose absence is currently correct.
- A slice reads `publish_time`, which would make it a parsed instant and raise the ordering question the
  provider's opt-in `orderingKey` exists to answer.
- `decode_with` gains a caller that needs the padding returned rather than re-derived, at which point `of` and
  `split_padding` would be reconciling one fact in two places and should become one.
