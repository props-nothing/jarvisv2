# ADR-0086: A refusal names the argument the caller sent, and a declared bound equals the one enforced

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0084` (a combination fault names which field conflicts), `ADR-0079` (a figure without
  its unit), `ADR-0035` (a documented invariant with no test is a convention), `ADR-0015` (cross-field
  consistency at construction).

## Context

Two defects, both about a value that appears in two places with nothing between them.

**First: a shared validator hard-coded the field name it reported.** `search_query` validated one string and
always said `field: "query"`:

```rust
fn search_query(value: &str) -> Result<&str, RequestError> {
    if value.chars().count() > MAX_QUERY_CHARS {
        return Err(RequestError::Argument { field: "query", … });
    }
    …
}
```

It had **three** callers:

| Caller | Argument | Correct field | What it reported |
| --- | --- | --- | --- |
| `gmail_messages_list` | `query` | `query` | `query` ✔ |
| `calendar_events_list` | `time_min` | `time_min` | **`query`** ✘ |
| `calendar_events_list` | `time_max` | `time_max` | **`query`** ✘ |

So an oversized `time_min` on `calendar_events_read` was refused with *"the `query` argument is unusable: a
search query may be at most 512 characters"* — naming an argument the tool does not have, and calling an
RFC 3339 instant a search query. A caller acting on that message would go looking for a `query` parameter that
does not exist. This is `ADR-0084`'s "remove the wrong argument" failure in a different place: there the fault
was a *combination* reported against one field, here it is one field reported against *another*.

**Second: the declared bounds and the enforced bounds agreed only by hand.** The input schemas declare
`maxLength`, `minLength`, `minimum` and `maximum` so a model is told the limits before it chooses an argument;
`request.rs` enforces the same limits through constants. Nothing tied the two, and one of them was missing
entirely — `time_min`/`time_max` were bounded in code but declared with **no** `maxLength` at all.

## Decision

The field name is an argument, the time bound gets its own validator, and a test ties every declared bound to
its constant.

1. **`search_query(field, value)` takes the field**, so the caller that knows which argument it is validating
   is the one that names it. The Gmail list passes `"query"`; nothing passes a field it does not know.

2. **A time bound gets its own validator, `time_bound(field, value)`, with its own bound and its own noun.**
   Not a reuse of `search_query` with a different field name, because the *reason* would still be wrong: the
   message said "a search query" for a value that is not one. The distinction is the same one
   `RequestError::DisallowedCombination` makes against `Argument` — the fault's *kind* belongs in the message,
   not only the field it is about.

3. **`MAX_TIME_BOUND_CHARS = 64`, not `MAX_QUERY_CHARS = 512`.** An RFC 3339 instant is about 25 characters
   and at most about 30 with fractional seconds and an offset, so 64 admits every valid instant with room to
   spare while still refusing a string that is plainly not a timestamp. The 512 bound was the query's, applied
   to a value of a different kind — a figure read as the wrong thing, which is `ADR-0079`'s defect.

4. **The schema declares `maxLength: 64` on both time bounds**, so the newly-added check is stated where a
   model reads it and not only enforced at call time.

5. **`every_declared_input_bound_matches_the_constant_that_enforces_it`** reads each declared bound out of the
   schema by JSON pointer and compares it to the constant. The comparison is against **the constants**, so it
   cannot be satisfied by editing the schema alone, and it covers all thirteen declared bounds plus the three
   `minLength` values.

## Consequences

- A refusal names the argument the caller sent. A model that sees "the `time_min` argument is unusable" can act
  on it; one told about a `query` it never supplied cannot.
- Every declared input bound is now **checked against the code that enforces it**. A change to either side that
  is not mirrored fails a test that names both, which is the property `ADR-0083` established for the output
  fields and `ADR-0085` for a token's two halves — the same "two values that must agree" defect, in a declared
  number.
- **Two mutants were falsified A-B-A**, both compiling:
  - hard-coding `field: "query"` in the oversized-bound refusal (detected by the field-naming test);
  - changing a declared `maximum` so it disagrees with its constant (detected by the drift test).
- **A limit remains:** `MAX_TIME_BOUND_CHARS` is **a JARVIS bound, not a provider figure** — Google publishes no
  length limit for `timeMin`/`timeMax`, so the constant is this crate's choice, which is why its doc says so.
  And the drift test asserts **equality**, so it proves the two statements agree, not that either is the right
  number: a bound changed in *both* places would pass with no provider evidence, which is why each constant
  carries its own justification rather than being asserted merely as "the value".

## Alternatives considered

- **Return the field name from the caller but keep one validator.** Done — that is decision 1. The alternative,
  a separate `time_query` function, would duplicate the control-character check for no gain.
- **Give `time_bound` the same 512 bound as a query.** Rejected: the bound would be a figure chosen for a
  different value, and the *reason* string would still have to explain a limit that does not fit a timestamp.
  A tighter, justified bound is also a better control, since nothing about an instant needs 512 characters.
- **Reuse `MAX_QUERY_CHARS` and rename it to something both can read.** Rejected: one constant for two kinds of
  value is the defect, not the fix — it would make "why 512?" unanswerable for either.
- **Rename `field` to `"time_min"`/`"time_max"` only, without adding the schema bound.** Rejected: the check
  would still be undeclared, so a model would send a 500-character bound, pass validation, and be refused at
  call time — the "declared but not enforced / enforced but not declared" asymmetry `ADR-0077` records.
- **Assert the drift with a literal per property.** Rejected: the test compares to the constants by name, so a
  literal copied into the test would drift with the constant and pass. Comparing to the constant is what makes
  the test about the *agreement* rather than about a third copy of the number.

## Conditions that would justify revisiting

- Google publishes a length limit for `timeMin`/`timeMax`, which would replace the JARVIS bound with a provider
  figure and change what the constant's doc must say.
- A new argument that is neither a search query nor a time bound is added, which would need its own validator
  rather than being run through either — the pattern this ADR establishes.
- The connector grows enough schemas that the explicit property list in the drift test becomes unwieldy, at
  which point the walk should become "every `maxLength`/`maximum` in every schema names a constant" rather than
  a table.
