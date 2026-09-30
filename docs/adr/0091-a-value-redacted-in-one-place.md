# ADR-0091: A value redacted in one place and printed in another

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0060` (a URL query is built from encoded parts — `HttpRequest` has no credential field),
  `ADR-0061` (the transport binding is the only place a token is read), `ADR-0089` (the envelope whose fields
  are redacted here), and `security.md`'s "no token material in model context, URLs/logs, diagnostics, or normal
  database columns".

## Context

The crate has an established way of hiding a value: a **hand-written `Debug`** that prints a marker, with a test
asserting the marker is present and the value is not. It appears at least three times, each with its own test:

| Type | Hidden field | What its `Debug` prints |
| --- | --- | --- |
| `AccessToken` (`credential.rs`) | the token | `value: [REDACTED], chars: N` |
| `FormRequest` (`request.rs`) | the credential-bearing form body | `body: [REDACTED], chars: N` |
| `VerifiedAccount` (`account.rs`) | `provider_account_id` | `provider_account_id: <redacted>` |

`VerifiedAccount`'s own comment says why: *"a provider account identifier is usually an email address and a
diagnostic that printed it would leak the account's owner."*

Then three later types **derived** `Debug` while holding values of the same kind:

| Type | Field it holds | What the derived `Debug` printed |
| --- | --- | --- |
| `PubsubNotification` | `email_address` — a mailbox | **the address** |
| `PubsubMessageBody` | `data` — base64 whose bytes decode to that address | **the payload** |
| `SyncCursor` | `token` — provider text addressing an account's data | **the token** |
| `SyncCursorParts` | the same `token`, moved here by `From<SyncCursor>` | **the token** |

**For `SyncCursor` the policy was already written down.** `DiagnosticField::CursorObservedAt`'s own doc says:

> "The **age** is what matters and an instant answers it; the token itself is never a field, because a cursor is
> provider-issued text that can address another account's data."

So the crate had decided that a cursor token must not appear in a diagnostic, and the type printed it under
`{:?}` — which is how a token reaches a log line. **A rule recorded in one module and violated in another is not
a rule**, and the reason the second module could violate it is that deriving `Debug` is the default and hiding a
field is a deliberate act with no shared mechanism making it visible.

`PubsubMessageBody`'s case is the subtlest, and worth separating from the other two: the field **looks like a
safe opaque blob**. It carries no credential, and a reader could reasonably conclude that printing a base64
payload is harmless. It is not — the bytes decode to `{"emailAddress": …}` — so the disclosure is the same one
`PubsubNotification` redacts, arriving through a field whose name gives no hint.

**A completeness sweep after fixing the three found a fourth, and it is the one that shows why fixing by module
is not enough.** `SyncCursorParts` is `SyncCursor`'s parts struct — the type `P3-006a` introduced so a token and
the account it came from travel together — and it **holds its own copy of the token**, moved in by
`impl From<SyncCursor>`. It derived `Debug` too. So after redacting `SyncCursor`, the very idiom this document
advertises as the safe way to move a cursor (`let parts: SyncCursorParts = cursor.into();`, exactly as the
existing test does) still printed the value, through a second `{:?}` on a type that is not the one the
redaction was written for. **A sibling type is not covered by its sibling's redaction**, and the way this was
found was by grepping for the *shape* (a `Debug`-deriving struct with a token-shaped field) rather than trusting
the three types already known to be wrong.

## Decision

Hand-written `Debug` on all four, following the crate's existing idiom.

1. **`PubsubNotification` redacts `email_address` and keeps `history_id`.** The asymmetry is the decision: a
   mailbox address names a person, and a history id is a *position* that names nobody — and it is exactly what
   an operator debugging a stuck sync needs to see.

2. **`PubsubMessageBody` redacts `data` and prints its length.** The marker carries `[REDACTED], N chars`,
   matching `AccessToken` and `FormRequest`, because "the payload arrived, and was about this big" is the useful
   diagnostic part and discloses nothing.

3. **`SyncCursor` redacts `token` and keeps kind, account, version and instant.** The account is an
   `AccountReference` — JARVIS's own local identifier, not the provider's id — so it names nothing about the
   mailbox.

4. **`None` prints as `None`, not as a redaction.** `SyncCursor::new` refuses a token on a `Start` cursor, so
   "this kind carries no token" is a fact worth seeing; a marker would make a start cursor look like a redacted
   one, which is the "two situations, one rendering" defect this repository keeps recording.

5. **`SyncCursorParts` redacts too, and the fix is in the type rather than in its callers.** The safe use of a
   type cannot depend on every caller choosing it: `Parts` exists to be moved around, and moving is exactly when
   the value is most likely to be printed while debugging. Adding a redaction one screen from the leak that the
   leak's own constructor produces is not a fix.

6. **`PubsubDelivery`'s derived `Debug` stays, with a comment saying why it is safe.** Its only sensitive field
   is inside `PubsubMessageBody`, whose `Debug` now redacts — so the derive prints a redaction. The comment
   records the check rather than the conclusion: **not** "does this struct hold a secret" but "does every field
   it holds refuse to print one", which is a property that changes when a neighbour changes.

## Consequences

- **The rule now holds in the modules that were violating it**, and each redaction has the test the crate's
  convention requires: the value is absent, the marker is present, and a **control** asserts the non-sensitive
  fields are still there — because a `Debug` that redacted everything would pass the first two assertions and
  make every diagnostic useless.
- **Two mutants were falsified A-B-A**, both compiling: printing the cursor token, and printing the mailbox
  address. Each was caught by exactly the test written for it, which is what makes the pair meaningful rather
  than a single test asserting a single string. Each of the four redactions has its own test in the crate's
  marker-plus-control shape.
- **A generalisation worth keeping:** `#[derive(Debug)]` is the safe default for *most* values and an unsafe one
  for any value that reaches a log, and a type cannot tell which it is. The crate's response is not to derive
  everywhere or never, but to hand-write where a field is sensitive and **test the marker** — so the decision is
  visible in the code and enforced by a test rather than remembered.
- **A second generalisation, from the fourth type:** **audit by shape, not by the list of known offenders.**
  Three types were found by reading a module; `SyncCursorParts` was found by grepping for `Debug`-deriving
  structs with a token-shaped field across the crate, which is a query that does not mention any of the three.
  Fixing the three and stopping would have left the leak that the *recommended* way to move a cursor produced.
- **A limit remains:** this is a *convention with tests*, not a mechanism. Nothing stops the next struct from
  deriving `Debug` while holding a sensitive field; what makes it a defect is that no test would fail, and the
  remedy applied here is a test per type rather than a lint over the crate. A `#[derive(Debug)]` on a struct
  whose field's `Debug` is redacted is also *transitively* safe, which is easy to assume and easy to get
  wrong — so the `PubsubDelivery` comment states the property to re-check rather than the conclusion.

## Alternatives considered

- **Omit the sensitive field from `Debug` entirely rather than redacting it.** Rejected: a reader cannot then
  tell "this type has no such field" from "a value was hidden", and the crate's existing idiom is the marker.
  `verified_account`'s `Debug` prints `provider_account_id: <redacted>` for the same reason.
- **Redact the whole value — a single opaque marker for the struct.** Rejected: it destroys the diagnostic value
  the marker-plus-control tests exist to protect. A `SyncCursor` that printed nothing would be useless to an
  operator, and `A10` requires diagnostics to "remain useful" as well as redacted.
- **Redact `SyncCursor`'s token but keep `PubsubMessageBody`'s `data`.** Rejected: the payload decodes to the
  address, so the two are the same disclosure. The reason `data` looked safe is that it is opaque *text*, and
  opacity is not safety.
- **Stop deriving `Debug` anywhere, to be safe.** Rejected: most types here hold nothing sensitive, and a
  hand-written `Debug` for each would be noise that hides the ones that matter.
- **Add a crate-level lint against `#[derive(Debug)]`.** Rejected as the wrong instrument: a lint cannot tell
  which fields are sensitive, so it would either forbid every derive or require an allow on nearly all of them.
  The convention-plus-test approach costs one test per sensitive type and is precise.
- **Change the types so the values are not held as `String`.** Rejected: the values are legitimately needed —
  the address identifies the mailbox in the notification, the token is the cursor, the payload is what is
  decoded — so the fix belongs in what they print, not in whether they exist.

## Conditions that would justify revisiting

- A crate-level mechanism appears that can mark a field as non-printable, at which point these hand-written
  impls should be replaced by it and the tests kept as the falsification.
- A sensitive value is added to a type whose `Debug` is derived *and* whose fields are not themselves redacted,
  which is the case these tests would not catch and which the `PubsubDelivery` comment asks the next reader to
  check.
- The address or the cursor token stops being sensitive — for instance if the notification is changed to carry
  an opaque mailbox id — at which point the redaction is preserving a constraint the provider no longer imposes.
