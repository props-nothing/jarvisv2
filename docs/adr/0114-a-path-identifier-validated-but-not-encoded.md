# ADR-0114: A path identifier that was validated but not encoded, and the asymmetry that hid it

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — making the Calendar path identifiers safe to put in a URL).
- **Relates to:** `ADR-0086` (the same "one validator, two kinds of value" question for time bounds),
  `ADR-0058` (the request module's own rule that a value which changes the request's *structure* is the
  injection it exists to prevent), `ADR-0112` (the watch builder this corrects beside its sibling), and
  `ADR-0107` (the sibling-asymmetry shape — two builders that should behave alike and did not).

## Context

`request.rs`'s module doc states the rule the module exists for:

> "**A query value is percent-encoded, and that is a security control rather than tidiness.** A Gmail search
> query is model-chosen text. Interpolated raw into `?q=…` it can contain `&`, `#`, or `=`, and the request that
> leaves is a *different* request than the one intended."

`gmail_messages_get` applies that rule to a **path** segment, with a comment saying so explicitly: *"The
identifier goes in the PATH, so it is percent-encoded as a path segment… a `/` in an identifier would change
which resource is addressed."* A shipped test asserts a `/` in a message id is escaped as `%2F`.

**Both Calendar builders put a `calendar_id` in the path and interpolated it raw.**

- `calendar_events_list` → `…/calendars/{calendar}/events`
- `calendar_channel_watch` → `…/calendars/{calendar}/events/watch`

A Gmail message id is an opaque hex string, so the raw form is *usually* indistinguishable from the encoded
one — which is why the omission was invisible. **A Calendar id is not that**, and its two common shapes both
contain URL-structural characters:

- a mailbox address — `primary`, `user@example.com`;
- a **holiday calendar** id — `en.usa#holiday@group.v.calendar.google.com`.

Interpolated raw, that `#` starts a URL **fragment** and truncates the path to `/calendars/en.usa` — a calendar
that does not exist. A `?` would turn the remainder into a query string, a `/` would change which path segment is
addressed, and a `%` would begin an escape sequence of the caller's choosing. **The request that leaves is a
different request than the one intended**, which is exactly the class the module's doc says the encoding exists
to prevent.

**And the asymmetry is what hid it.** `gmail_messages_get` encoded; the two Calendar builders did not. A reader
checking "does this module encode path identifiers?" finds a `yes` — in the sibling that was written with a
message id in hand. The defect is only visible by comparing the builders **side by side**, which is the same
shape `ADR-0107` records for two teardown steps that should have matched and did not.

**The validator's own doc overclaimed, which is the second half.** `resource_id`'s doc said the
control-character refusal *"matters beyond a malformed URL"* — true of **log forging** — in a way that read as
covering URL safety. It does not: the validator deliberately **accepts** the URL-structural characters a real
calendar id contains (`@`, `#`, `%`, `/`), because refusing them would make a conforming identifier unusable.
So the check addresses one risk and the encoding addresses a **different** one, and a doc that implied the first
covered the second is what let a builder omit the second.

## Decision

**1. Both Calendar path builders percent-encode the calendar id, through the same `percent_encode`.**

```rust
format!("{CALENDAR_API_BASE}/calendars/{}/events", percent_encode(calendar))
format!("{CALENDAR_API_BASE}/calendars/{}/events/watch", percent_encode(calendar))
```

The same function `gmail_messages_get` uses, so there is **one** encoder for query values and path segments —
the unreserved set is identical (`ALPHA / DIGIT / "-" / "." / "_" / "~"`), which is why the same routine is
correct in both positions. No new helper: a second encoder for paths would be two implementations of one rule.

**2. The `resource_id` doc is corrected rather than left to imply coverage it does not have.**

It now states what the control-character check **does** address (log forging) and what it deliberately does
**not** (request structure), and names `percent_encode` as the control for the latter. A validator that made an
identifier URL-safe would have to refuse `#`, `?`, `/` and `%`, and a calendar id contains those — so the split
is a property of the values, not a convenience.

**3. The watch builder's correction is recorded as the more consequential half.**

A wrong path on a **read** addresses the wrong calendar and fails. A wrong path on a **watch** registers a
notification channel against the wrong resource — and the channel's id is then the join key a delivery routes
on, so the mistake propagates past the call. Both are fixed by the same one-line change, but the comment on the
watch builder says why it matters more.

## Consequences

- **The Calendar builders now match their Gmail sibling.** A `/`, `#`, `?` or `%` in a calendar id is escaped,
  so a holiday calendar or a mailbox-address id addresses the resource the caller named.
- **The `primary` alias is unchanged**, and asserted: `percent_encode` leaves an already-safe id alone, so the
  control does not percent-escape every ordinary Calendar read.
- **The module's encoding rule is no longer "query values only" in practice.** The rule was always about the
  *structure-changing characters*, and a path segment is a second place they occur; the Gmail builder and the
  two Calendar builders now apply it identically.
- **Two guards falsified A-B-A**, both compiling: (1) removing the encoding from `calendar_events_list` →
  **detected** (`…/calendars/en.usa#holiday@…/events`, the hash intact); (2) removing it from
  `calendar_channel_watch` → **detected** (`…/en.usa#holiday@…/events/watch`).
- **The test asserts both halves of the id and both builders**, so a partial fix — escaping only the `#` in only
  one builder — fails. Each structural character is asserted as its **exact** encoding (`#`→`%23`, `?`→`%3F`,
  `/`→`%2F`, `%`→`%25`) rather than by the absence of the raw character, because a test that only checked
  "no `#` in the URL" would pass for a builder that dropped the character entirely.

## Alternatives considered

- **Refuse the URL-structural characters in `resource_id` instead of encoding them.** Rejected: a calendar id
  contains `#` and `%` legitimately (a holiday calendar's id is `…@group.v.calendar.google.com` with a literal
  `#`), so refusing them would make a conforming identifier unusable — the same "a validator that is stricter
  than the provider" direction `ADR-0086` records for time bounds. The value must be **encoded**, not refused.
- **Add a separate `percent_encode_path` helper.** Rejected: the unreserved set is identical for a query value
  and a path segment, so the function is already correct in both positions and a second one would be two
  implementations of one rule that could drift — the defect `ADR-0071`/`ADR-0072` record for two form encoders.
- **Encode in a URL-building helper that all builders share.** Considered and left out of this slice: it would
  be the right shape for a *fourth* builder, and the two here plus the Gmail one are now identical, so the
  extraction is a refactor rather than a fix. It is named as a revisit condition instead of done speculatively.
- **Leave the `resource_id` doc as it was and only fix the builders.** Rejected: the doc's implication is what
  made the omission plausible, and a reader who trusts "the validator handles it" will omit the encoding in the
  next builder. The doc is corrected in the same change that fixes the call sites.

## Conditions that would justify revisiting

- **A fourth builder puts an identifier in a path or a query**, at which point the encoding should be extracted
  into a URL-building helper so the decision is made once — and the test that asserts each builder's encoding is
  the check that the helper is used.
- **A Calendar identifier is accepted from a model-facing tool** (`P5-009`'s write operations would name a
  calendar), at which point the input schema is the first control and this encoding is the second; the two must
  agree about which characters are permitted.
- **A Google API documents an identifier that may contain a `/` as a genuine path separator** — no current
  identifier does, and the encoding would then need to be per-segment rather than whole-value.
