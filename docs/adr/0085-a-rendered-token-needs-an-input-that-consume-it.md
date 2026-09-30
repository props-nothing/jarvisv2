# ADR-0085: A token the output renders needs an input that can consume it

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0083` (a declared output field is bounded by what the request can return — this is that
  bound checked across the two *halves* of one tool), `ADR-0084` (the same symmetry on the input side),
  `ADR-0082` (a declaration nothing produces), `ADR-0060` (`CalendarPage` keeps its two tokens apart).

## Context

`calendar_events_read` rendered `next_page_token` and declared it in its output schema:

```json
"properties": {
  "event_ids": { "type": "array", "items": { "type": "string" } },
  "next_sync_token": { "type": ["string", "null"] },
  "next_page_token": { "type": ["string", "null"] }
}
```

A test asserted the token arrived on a mid-walk page, and the renderer emitted it. But the tool's **input**
declared no `page_token`, and `calendar_events_list` never sent `pageToken`:

```rust
pub fn calendar_events_list(
    calendar_id: &str,
    time_min: Option<&str>,
    time_max: Option<&str>,
    max_results_value: Option<u32>,
    sync_token: Option<&str>,          // ← no page token
) -> Result<HttpRequest, RequestError>
```

So the tool told the caller "there is a next page" and gave it **no argument to fetch it with**. The two Gmail
list operations both pair `next_page_token` out with `page_token` in (`ADR-0060`); Calendar had out only.

**And the missing page is the common case, not an edge one.** The sync guide, verbatim:

> "In cases where a large number of resources have changed since the last incremental sync request, you may find
> a `pageToken` instead of a `syncToken` in the list result. In these cases you'll need to perform the exact same
> list query as was used for retrieval of the first page in the incremental sync (with the exact same
> `syncToken`), append the `pageToken` to it and paginate through all the following requests until you find
> another `syncToken` on the last page."

A sync of a busy calendar therefore returns a page token instead of a cursor, and the walk cannot finish without
it. The connector could not reach the end of such a sync at all.

## Decision

`calendar_events_list` takes the page token, and a test holds the two halves in step.

1. **The builder gains `page_token` and sends `pageToken`,** validated by `client::next_page` — the same bound
   every token goes through, so the new parameter is not a hole in the existing validation.

2. **A page token and a sync token are sent *together* for the middle pages**, and that is the opposite of the
   `timeMin`/`timeMax` restriction (`ADR-0084`). The two rules are different and only one is a conflict: the
   sync guide's own pagination example is `…&syncToken=…&pageToken=…`. The test asserts both are present, so a
   change that made the page token displace the sync token — which would restart the walk against a full sync —
   fails.

3. **The input schema gains `page_token`**, and its description names why the two tokens coexist: "the same
   query must be repeated with it — including the same `sync_token`, which is how a large incremental sync is
   walked." A model reading only "continues a paginated read" would not know the sync token must be repeated
   with it, and the guide is explicit that the *exact same* query is required.

4. **The pairing is asserted across the tool's two halves, not per tool.** The test walks every definition and
   asserts that any whose output declares `next_page_token` also accepts `page_token` as input. Each schema was
   internally consistent, which is why nothing caught this; only the pairing exposes it. This is `ADR-0083`'s
   method applied across the input and output halves of one tool, so a future paginated read cannot ship
   one-directional.

## Consequences

- A large incremental sync can now be walked to its sync token. Before, the connector could return page one of a
  large change set and had no way to reach page two — so the cursor the whole sync mechanism exists to advance
  was unreachable precisely when it mattered.
- The two Calendar continuation tokens are now both **round-trippable**: `next_page_token` out pairs with
  `page_token` in, and `next_sync_token` out pairs with `sync_token` in. `ADR-0060` established they are
  different things; this makes both usable.
- **Two mutants were falsified A-B-A**, both compiling:
  - removing the builder's `pageToken` push (detected by the builder test and the operation-layer test);
  - removing `page_token` from the input schema (detected by the reverse pairing test alone).
- **A limit remains:** the walk is *possible* but nothing performs it. No code loops on `next_page_token`, so a
  caller must issue the follow-up call itself — the multi-page walk is `P5-010`'s "pagination" work and the
  connector currently implements the *ability* to paginate rather than an automatic walk. Also unverified: no
  request has been sent, so what `events.list` returns for a large change set is taken from the guide and not
  observed.

## Alternatives considered

- **Leave it out, since single-page reads work.** Rejected: the mid-walk page token is produced by the provider
  for a large sync, and a caller that cannot use it cannot finish the sync. The declaration and the renderer
  already promised the token; the missing input was the defect, and removing the output field instead would have
  hidden a real capability the guide documents.
- **Rename the output rather than add the input.** Rejected: `next_page_token` is what the field is, and
  `ADR-0060`'s distinction from `next_sync_token` is exactly the information a caller needs. The fix belongs on
  the input side.
- **Make the page token positional and reuse the Gmail builder's shape exactly.** Done — the parameter order
  matches `gmail_messages_list` (`max_results` then `page_token`), so the two list builders read alike and a
  reader comparing them sees the same shape.
- **Assert the pairing with a per-tool literal for Calendar only.** Rejected: the defect was a *pairing*, and a
  per-tool assertion would not catch the next tool that ships one-directional. The loop over every definition
  costs the same to write and generalises, matching the reverse test `ADR-0083` introduced.
- **Add a conflict check for `page_token` + `sync_token` by analogy with `ADR-0084`.** Rejected explicitly: they
  do **not** conflict — the guide requires them together — and adding a check by shape rather than by the
  provider's own list would refuse the documented walk, which is the failure `ADR-0084`'s "read each parameter's
  own entry" rule prevents.

## Conditions that would justify revisiting

- A slice implements the automatic walk, at which point "the caller must loop" stops being a limit and the loop
  becomes the tested behaviour.
- The provider adds a parameter to the disallowed-with-`syncToken` list that the connector sends, which would
  need a check in the builder beside the time-range one.
- A live call shows `pageToken` behaving differently from the guide (for example not requiring the sync token),
  which would falsify the "exact same query" reading and change the input's description.
