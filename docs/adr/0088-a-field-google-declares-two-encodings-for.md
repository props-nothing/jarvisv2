# ADR-0088: A field Google declares two encodings for

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0063` (a fixture declares whether it is a capture or a shape), `ADR-0087` (the watch
  lease, whose renewal this notification path feeds), `ADR-0074` (a name that asserts something the code does
  not do), and the "prefer the reading whose error repairs itself" rule in `docs/development`.

## Context

The Gmail push delivery path needs to read the `message.data` field of the Cloud Pub/Sub `PubsubMessage` that
arrives as a `POST` body. Two official pages describe that field's encoding, and **they do not agree**:

- The **Gmail push guide**: *"The `message.data` field is a **Base64URL**-encoded string that decodes to a JSON
  object containing the email address and the new mailbox history ID."*
- The **`PubsubMessage` reference** — the type the same guide links to — declares the field
  `data | string (bytes format) | … A **base64**-encoded string.`

RFC 4648 §4 (standard, `+`/`/`) and §5 (URL-safe, `-`/`_`) differ in exactly two characters, so the
disagreement is **invisible on any value that contains neither**. That includes the guide's own example:

```
eyJlbWFpbEFkZHJlc3MiOiAidXNlckBleGFtcGxlLmNvbSIsICJoaXN0b3J5SWQiOiAiMTIzNDU2Nzg5MCJ9
```

which decodes (identically under both alphabets) to
`{"emailAddress": "user@example.com", "historyId": "1234567890"}` — exactly what the guide says.

**And in practice almost no notification can tell them apart.** A sweep of all 95 printable ASCII characters at
all four base64 alignments, in a Gmail-shaped JSON payload, found only **three** whose standard encoding
contains `+` or `/`: `>`, `?` and `~`, each only after a one-character offset. So the two readings agree on
nearly every real payload, which is precisely why the contradiction is worth recording rather than resolving
by preference: it is invisible until the one value that differs — and on that value, choosing the wrong
alphabet means **refusing a delivery**, which is a **missed change**.

## Decision

The decoder accepts **either** declared encoding, tries URL-safe first, and **reports which one matched**.

```rust
pub enum PubsubData { UrlSafe, Standard }

pub fn decode_notification(data: &str)
    -> Result<(PubsubNotification, PubsubData), PubsubNotificationError>
```

**URL-safe first**, because the Gmail push guide is the more specific statement — it describes *this*
notification payload — while the `PubsubMessage` reference describes a general Cloud Pub/Sub field that Gmail
happens to reuse. Preferring the specific statement is the reading that matches the surface being implemented.

**Both are tried**, because the contradiction is the provider's and neither reading is provably wrong. A
decoder that accepted only one would refuse a delivery encoded the other way, and a notification the connector
refuses is a missed change — the silent failure `ADR-0087` exists to remove. Accepting both costs one extra
attempt on a value whose alphabet was already wrong.

**The matched encoding is returned, not discarded.** That is what keeps the contradiction *visible*: a caller
that logged the [`PubsubData`] from a real delivery could settle the question from evidence, where a decoder
that silently picked one would have resolved an open question in the provider's favour without saying so.

Three supporting decisions:

1. **Padding is refused, not tolerated.** Neither form this crate accepts is padded (URL-safe because RFC 7636
   requires padding omitted; standard because the caller passes what its encoder produced). A decoder that
   quietly stripped `=` would accept two spellings of one value and could not say which it received.
2. **An empty `data` is refused as a shape, not read as "nothing changed".** The `PubsubMessage` reference
   permits an empty `data` if attributes are present, but a *Gmail* notification with no payload is a delivery
   this connector does not understand — and reading it as an empty change set would advance nothing and say
   nothing, which is the silent direction.
3. **Four distinct errors**, because the remedies differ: not base64 under either alphabet, not UTF-8 after
   decoding, and not the documented JSON object (with the missing field named by absence). Each points a reader
   at a different layer.

## Consequences

- The connector can read a push delivery without betting on which of two official sentences is authoritative.
- **The contradiction is recorded rather than resolved**, with the evidence that makes it hard to see: the
  guide's own example decodes identically under both alphabets, and a printable-ASCII sweep found only three
  characters that force the difference.
- Base64 gained one home. The encoder was a private function in `auth.rs`; a second caller moved it to
  `crate::base64`, **unchanged**, so the RFC 7636 Appendix A test that reaches it through `PkceVerifier` still
  guards the same lines. The decoder is new, and `standard_padded` was written and then **removed** as unused —
  a dead encoder is a second implementation that nothing checks.
- **Two mutants were falsified A-B-A**, both compiling:
  - removing the standard-alphabet fallback, so only URL-safe decodes (detected by the alphabet test and the
    padding test);
  - stripping padding instead of refusing it (detected by the padding test, which names the padding).
- **A limit remains:** no delivery has been received, so which encoding a live subscription actually uses is
  **unknown** — that is the whole point of reporting [`PubsubData`], and it stays open until a real delivery is
  logged. The Pub/Sub envelope as a whole is also not parsed: the module decodes `message.data` and does not
  read `messageId`, `publishTime`, `subscription` or the attributes map, none of which this path needs yet.

## Alternatives considered

- **Accept only URL-safe, following the Gmail guide.** Rejected: it would refuse a delivery encoded the other
  way, and a refused notification is a missed change. The guide's specificity makes URL-safe the *first*
  attempt, not the only one.
- **Accept only standard, following the `PubsubMessage` reference.** Rejected for the same reason, with the
  additional note that the general reference is the weaker statement about a Gmail-specific payload.
- **Try standard first.** Rejected only on specificity: the reported encoding would then differ for every
  payload whose characters overlap (which is nearly all of them), making the *report* less informative even
  though both decodings are identical.
- **Strip padding silently.** Rejected: it accepts two spellings of one value, and this crate's rule is that a
  decoder says what it received. Padding is also a signal about which of the two documented forms a producer
  intended.
- **Treat an empty `data` as an empty notification.** Rejected: it turns a delivery the connector does not
  understand into a statement that nothing changed — the failure mode where a mailbox silently stops syncing.
- **Generate the forcing fixture with a second encoder.** Rejected: a second encoder is a second thing that can
  be wrong, and the fixture is clearer spelled out. The standard spelling is derived from the URL-safe one by
  the RFC 4648 §5 substitution, and the test asserts the substitution actually changed the text.
- **Resolve the contradiction by treating the guide as authoritative and noting it.** Rejected: the note would
  live in a comment while the code bet on one answer, and the code is what runs.

## Conditions that would justify revisiting

- A real delivery logs a [`PubsubData`], which would settle which encoding the provider uses and let the other
  branch become a deliberate compatibility affordance rather than an open question.
- Google corrects one of the two pages, which would make the contradiction a resolved fact and the second
  attempt vestigial — recorded, but no longer load-bearing.
- The path grows beyond `message.data` — a `messageId` for deduplication, a `publishTime` for ordering, the
  attributes map for filtering — at which point the envelope becomes a type rather than one decoded field.
- A second consumer of `crate::base64` needs the **padded** standard form, at which point `standard_padded`
  should return with a caller rather than as an unused function.
