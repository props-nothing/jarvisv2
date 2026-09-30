# ADR-0084: An argument pair the provider forbids is refused before it is sent

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0082` (a `400` is a permanent caller error), `ADR-0083` (a declaration bounded by what
  the request can return — this is the same bound on the *input* side), `ADR-0077` (a bound documented but not
  applied is not a bound), `ADR-0015` (tool-contract consistency checked at construction).

## Context

`calendar_events_read` accepted three arguments — `time_min`, `time_max` and `sync_token` — and the request
builder sent all three together when a caller supplied them:

```rust
push(&mut parameters, "timeMin", …);
push(&mut parameters, "timeMax", …);
push(&mut parameters, "syncToken", …);
```

The `events.list` reference says this request cannot succeed. Under `syncToken`, verbatim:

> "There are several query parameters that **cannot be specified together with nextSyncToken** to ensure
> consistency of the client state. These are: `iCalUID`, `orderBy`, `privateExtendedProperty`, `q`,
> `sharedExtendedProperty`, **`timeMin`**, **`timeMax`**, `updatedMin`."

The sync guide corroborates and explains why: a time range belongs to the **initial full sync**, and each
incremental sync must "use the same set of query parameters, including the initial request" — the guide's own
sample sets `timeMin` only in the full-sync branch and `syncToken` only in the incremental one. The same page
states the consequence: "The response code for list queries containing disallowed restrictions is `400`."

So the connector declared an input schema in which **two of its own fields could be combined into a request
the provider documents as a `400`**, and the caller would have learned this only by spending a request.

## Decision

The pairing is refused locally, and the schema says why.

1. **`request::calendar_events_list` refuses `sync_token` with either time bound**, before building anything.
   `RequestError::DisallowedCombination { field, other, reason }` is a **new variant** rather than a reuse of
   `Argument`: both values may be individually valid and the *combination* is what the provider rejects, which
   is a different class of fault and needs a different message. A reader told "`time_min` is unusable" would
   fix the wrong thing.

2. **The refusal names which bound conflicts.** The two checks are separate, so a caller who sent only
   `time_max` is told about `time_max` rather than about "a time range". The message also states both remedies
   — drop the bounds to continue the sync, or drop the token to do a filtered full read — because the caller
   has a genuine choice and not just a mistake to undo.

3. **The input schema carries the restriction twice.** The `time_min`, `time_max` and `sync_token` descriptions
   state it, because **the schema is what a model reads** and a model that saw both fields advertised with no
   note could reasonably choose the combination. And an `allOf`/`not` constraint refuses it for a validator:

   ```json
   "allOf": [
     { "not": { "required": ["sync_token", "time_min"] } },
     { "not": { "required": ["sync_token", "time_max"] } }
   ]
   ```

   Both are present because they serve different readers — a model reads prose, a validator reads the document —
   and this repository's rule is that a documented invariant with no check is a convention (`ADR-0035`).

4. **The full sync keeps its filters.** A filtered full read (`time_min` + `time_max`, no token) remains legal
   and is asserted so: it is what the sync guide's own sample does, and a schema that rejected every time bound
   would pass a naive "the pair is refused" test.

## Consequences

- A request the provider documents as impossible can no longer be built, so the failure is a local refusal with
  two named remedies instead of a `400` with a machine-readable reason the caller must interpret.
- The declared input schema and the request builder **agree about the combination**, which is the property
  `ADR-0083` establishes for the output side. The schema test asserts rejection by the *document*, so removing
  the `allOf` while leaving the prose fails — the "documented but not applied" defect.
- **One provider restriction is now recorded in one place and referenced.** The `syncToken` note lists **eight**
  parameters; the builder can send two of them (`timeMin`, `timeMax`). The other five — `iCalUID`, `orderBy`,
  `privateExtendedProperty`, `q`, `sharedExtendedProperty`, `updatedMin`, and `nextSyncToken` itself — are
  **not offered by this operation at all**, which is why they need no check: a parameter the connector never
  sends cannot be sent with a token. The builder's doc records this, so adding one of them later is a
  deliberate act against a written list rather than an oversight.
- **Two mutants were falsified A-B-A**, both compiling:
  - removing the builder's refusal (detected by the request test and the operation-layer test);
  - removing the schema's `allOf` while leaving the descriptions (detected by the schema test — the one that
    proves the constraint is enforced and not merely documented).
- **A limit remains:** the restriction is taken from the reference and has not been observed, so if Google
  accepts a time range with a sync token in practice the connector refuses a call that would have worked. That
  direction is the safe one — the sync guide's own sample never combines them — but it is a limit the live smoke
  test would close. Nothing outside this module consumes `DisallowedCombination` yet, since no composition root
  builds the connector; `request_for` is the only caller and the tests are what hold the behaviour in place.

## Alternatives considered

- **Send the request and let the provider answer `400`.** Rejected: `ADR-0082` already classifies a `400` as
  `Permanent`/`DoNotRetry`, so the outcome would be identical except that it cost a request, a quota unit, and a
  round trip to learn something the reference states — and the caller would still have to work out which
  argument to drop.
- **Silently drop the time bounds when a sync token is present.** Rejected: that would answer a question the
  caller did not ask. A caller who supplied both meant something by the time range, and discarding it silently
  is the class of defect this repository records repeatedly — an input quietly ignored.
- **Silently drop the sync token instead, doing a filtered full read.** Rejected for the stronger reason: it
  would turn an incremental sync into a full one, which is a **different amount of work** and would make a
  caller believe a sync had advanced when it had not.
- **Reuse `RequestError::Argument` for the refusal.** Rejected: it names one field and one reason, and this
  fault is about a pair. A caller told `time_min` was unusable would remove the wrong argument — the bounds are
  the legitimate part of a full sync.
- **Constrain the schema only** (no builder check). Rejected: a schema constrains a *model's* arguments, not
  every caller — the operation layer can be reached without going through schema validation, so the builder is
  where the invariant must hold. The schema is the early, friendly signal; the builder is the guarantee.
- **Constrain the builder only** (no schema constraint). Rejected: a model choosing the combination would get a
  refusal it could have been warned about, and the description would be a promise nothing enforced.

## Conditions that would justify revisiting

- A live call shows Google accepting a time range with a sync token, which would falsify the restriction and
  make this refusal wrong.
- Another operation needs a disallowed pair, which would make `DisallowedCombination` a table rather than two
  inline checks — and the eight-parameter list is the shape such a table would take.
- A slice adds `q`, `orderBy`, `updatedMin`, `iCalUID` or either extended-property filter to `events_read`,
  each of which joins the disallowed list and would need its own check against the same note.
