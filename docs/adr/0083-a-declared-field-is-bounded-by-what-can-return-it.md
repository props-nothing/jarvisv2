# ADR-0083: A declared output field is bounded by what the request can return

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0059` (a tool definition is derived from the manifest, so the declared schema is the
  contract), `ADR-0082` (a declaration nothing produces — the same defect in the *input* direction), `ADR-0063`
  (a fixture declares whether it is a capture or a shape).

## Context

`gmail_messages_read` declared **four** top-level output fields and its renderer produced **one**.

The declared schema, before this change:

```json
"properties": {
  "message_id": { "type": "string" },
  "thread_id":  { "type": "string" },
  "label_ids":  { "type": "array", "items": { "type": "string" } },
  "snippet":    { "type": ["string", "null"] }
},
"required": ["message_id"]
```

The renderer, before this change:

```rust
"gmail_messages_read" => request::parse_single_id(response.status, &response.body)
    .ok()
    .map(|id| serde_json::json!({ "message_id": id }).to_string()),
```

So **three of the four declared fields were unreachable**. A consumer reading the schema the connector
publishes would find `thread_id`, `label_ids` and `snippet` promised, and no response would ever carry them.
The existing test suite did not catch it because `every_rendered_output_satisfies_the_schema_the_definition_declares`
validated *the rendering against the schema* — and a rendering of `{"message_id":"m1"}` is a perfectly valid
instance of a schema that only requires `message_id`. **A validity check is one-directional**: it proves the
rendering is inside the schema, never that the schema's fields are inside the rendering.

A second defect sat underneath, and it is the one that made the fix a *removal* rather than a pure addition.
The `Message` resource does have a `snippet` field, so the declared `snippet` looked deliverable. But the
**Format** page defines what each format returns:

> - `minimal` — "Returns only email message ID and labels; does not return the email headers, body, or payload."
> - `metadata` — "Returns only email message ID, labels, and email headers."
> - `full` — "Returns the full email message data with body content parsed in the payload field…"

`gmail_messages_read` offers all three formats, with `full` as the default. So `snippet` — "A short part of
the message text" — is **not returned by two of the three formats the tool accepts**. A declared-`snippet`
field would therefore be undeliverable for exactly those two, and a model choosing `minimal` for cost would
receive an output that does not match the tool it called.

## Decision

The parser returns the fields the output declares, the renderer emits them, and the schema declares only what
the request can return.

1. **A private `MessageBody` and a public `GmailMessage`.** `parse_single_id` — which returned one `String` —
   becomes `parse_single_message`, returning `GmailMessage { id, thread_id, label_ids }`. The type is **not**
   Gmail's `Message` resource; it carries exactly the top-level fields this tool's output promises, the same
   rule `IdPage` and `CalendarPage` already follow, so the declaration and the rendering cannot drift.

2. **`snippet` is removed from both the schema and the type.** The fix for "a declared field the request cannot
   return" is not to declare it optional or to fetch it with a second request: it is to **stop declaring it**.
   The tool offers `minimal` and `metadata`, which cannot return `snippet`, so the honest contract omits it. A
   `full` read does carry it, and the day a tool needs it the format is the lever — not a field that is absent
   for two of three formats.

3. **`thread_id` and `label_ids` are optional; only `message_id` is required.** `minimal` and `metadata` may
   not carry `threadId` (the Format page names `labelIds` for both and says nothing about `threadId`), so a
   `required` field a `minimal` read cannot fill would make an honest response fail the tool's own validation.

4. **An absent list is not an empty list.** `label_ids` is `Option<Vec<String>>`, and the renderer emits the
   key **only when the provider returned it**. `None` means "the field was not in the response"; `Some(vec![])`
   means "the message carries no labels". A `Vec` with `#[serde(default)]` would collapse both to `[]` and
   render "this message has no labels" for a response that never mentioned labels — the two-values-three-
   situations defect this repository keeps recording.

5. **The reverse direction is now a test.** `no_declared_output_property_is_undeliverable` compares the
   declared property **names** against the keys of a maximal rendering, so a field added to the schema and
   forgotten in the renderer fails here. This is the check that was missing: the forward test proves the
   rendering is valid, and only the reverse proves every declared field is producible.

## Consequences

- The output contract is delivered. A consumer reading `message_id`, `thread_id` or `label_ids` from the
  schema will find all three in a `full` response.
- The tool's declared output is **smaller and true** rather than larger and partly fictional. `snippet` is
  gone, which is a visible reduction in what the schema advertises — the correct direction, because the
  alternative was advertising a field the tool could not reliably return.
- **The one-directional validity check is now two-directional**, and the reverse test is written as a
  comparison of name sets rather than a per-field assertion, so it does not need updating when a field is added
  — it fails until the renderer catches up, which is the point.
- **Two mutants were falsified A-B-A**, both compiling:
  - reverting the renderer to emit only `message_id` (the original defect: detected by three tests);
  - re-adding an undeliverable declared field to the schema (`snippet`: detected by the reverse test).
- **A limit remains:** no request has been sent, so what `minimal` and `metadata` actually return is taken from
  the Format page and not observed. If `minimal` returns more than "ID and labels" — or `threadId` for any
  format — the optionality here is looser than reality, which fails safe (an optional field that is always
  present validates) but is still a gap the live smoke test would close. And `GmailMessage` models three fields
  where the resource has nine; a future read wanting `historyId` or `internalDate` adds them to the type and
  the schema together, which is the pairing this ADR makes visible.

## Alternatives considered

- **Add `thread_id` and `label_ids` to the renderer and keep `snippet`.** Rejected: `snippet` is not returned by
  `minimal` or `metadata`, so keeping it would leave one undeliverable field and the same defect at three
  quarters scale.
- **Make `snippet` optional and render it when present.** Rejected: for a field two of three formats never
  return, "optional" reads as an unreliable field rather than an impossible one, and a model cannot tell which
  it is. Absence from the contract is the honest signal.
- **Ask for `full` always, so every field is deliverable.** Rejected: `minimal` exists to read cheaply, and
  forcing `full` would make the connector ignore the caller's cost intent — a `minimal` read is a lower quota
  cost and less data, which is the whole reason the format argument exists.
- **Derive the output schema from `GmailMessage` rather than writing it as a constant.** Considered and not
  taken here: the schema is the *contract* and is meant to be readable as a document, and the reverse test now
  holds the two in step, which is what derivation would have bought. A future slice may revisit it if the
  field set grows.
- **Make the schema-match check assert required-field presence instead of a name-set equality.** Rejected: it
  would pass for an optional declared field the renderer never emits, which is precisely the `snippet` case.

## Conditions that would justify revisiting

- The live smoke test shows a `minimal` read carrying `threadId`, which would make it required rather than
  optional.
- A slice needs `snippet`, which would mean offering it **only for `full`** — a format-conditional field the
  schema cannot express today, and a decision about whether the output should vary by request.
- Another operation's declared output is found to promise a field the renderer does not produce, which would
  mean the reverse test should run for every operation and not only this one (the current reverse test is
  written for `gmail_messages_read` because that is where the defect was).
