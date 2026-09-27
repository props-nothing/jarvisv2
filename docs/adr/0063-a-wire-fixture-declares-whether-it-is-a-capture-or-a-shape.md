# ADR-0063: A wire fixture declares whether it is a capture or a shape

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google connection's recorded wire fixtures).
- **Relates to:** `ADR-0056` (a checklist item is evidence of a kind), `ADR-0036` (a self-reported name is
  evidence or nothing), `ADR-0059` (a tool definition is derived from the connector's manifest), `ADR-0062` (an
  unanswered request is classified by whether it may have reached the provider).

## Context

`P5-005`'s acceptance text is "Implement Google connection setup and Gmail/Calendar read tools **with recorded
wire fixtures**". And `docs/development/external-research.md` says to "validate against official schemas or
sanitized real wire fixtures and include an opt-in live smoke test where provider behavior cannot be proven
offline."

At the point this slice reached, the connector had a contract, an auth flow, a client, derived tool
definitions, a request builder, a credential boundary, a transport port, and an operations layer — and **no
request had ever been sent**, because there is no transport implementation, no credential, and no account.
So the fixtures had to be written **before** any capture was possible, and the honest options were:

1. Write no fixtures, and leave `P5-005` blocked on a live call that cannot happen yet.
2. Write fixtures built from the published reference pages, and call them "recorded wire fixtures" because
   that is what the acceptance text says.
3. Write fixtures built from the published reference pages, and **say plainly in the artifact and in the test
   that nothing was recorded**.

Option 2 is the failure mode this ADR exists to prevent, and it is a real one: a directory named
`tests/fixtures/google/` containing JSON that parses cleanly looks exactly like a recording. The word
"fixture" does not distinguish a payload copied from a live response from one assembled out of documentation,
and both satisfy "we have fixtures for that operation". The confusion is not hypothetical — `ADR-0056` records
the same shape for checklists, where a scaffold that invented an answer produced a document that read as
completed.

There is a second, sharper problem, and it is what actually produced this ADR. Google documents
`nextPageToken` and `nextSyncToken` on `events.list` as **mutually exclusive**:

> `nextPageToken` … "Omitted if no further results are available, in which case nextSyncToken is provided."
> `nextSyncToken` … "Omitted if further results are available, in which case nextPageToken is provided."

**An earlier version of this slice's own test asserted a body carrying both.** It passed, because the parser
accepts both independently — and the input it used is one Google cannot produce. The defect was invisible to
the test that covered it, and it was found by reading the **field descriptions** rather than the example JSON.
That is the general failure of a hand-built payload: it is easy to make internally consistent and impossible to
make realistic in its *constraints*, because the constraints live in prose ("omitted if…") rather than in the
sample.

## Decision

**1. A fixture declares its provenance in the data, not only in a directory name or a comment.**

Every fixture carries `_not_a_capture: true` and `_shape_documented_at: <url>`, in the file itself.

**2. The declaration is asserted, so it cannot be dropped silently.**

`assert_declared_shape` checks the marker on every fixture it reads, and a sweep asserts it on every `*.json`
in the directory. A file that lost the marker fails the suite rather than passing as an apparent recording.
This is the same rule as `ADR-0056`'s scaffold and `ADR-0036`'s self-reported name: **the artifact must not be
able to claim more than it has.**

**3. A fixture states its own point, because a reader cannot recover it from the bytes.**

`_the_point_of_this_fixture` explains what the payload is *for* — for example that the Calendar pair exists
because the two continuation tokens are mutually exclusive, and that the two 403 fixtures exist as a **pair**
whose only difference is the reason code. Without that, a maintainer "simplifying" the pair into one file would
silently delete the only test that distinguishes a throttling limit from a disabled application.

**4. Constraints are tested as separate states, not as one convenient payload.**

Where a provider's fields are mutually exclusive, the fixtures are the **states** (a mid-walk page, a last
page), not a union of them. A union is unrepresentable at the provider and therefore tests nothing that can
happen.

**5. A fixture's provenance claim is falsifiable, and it was falsified.**

Two mutants, both compiling: flipping `_not_a_capture` to `false` (detected by 3 tests) and deleting
`nextPageToken` from the mid-walk page (detected by 3 tests). The second is the important one — it shows the
mutual-exclusion property is genuinely load-bearing rather than incidentally true.

**6. The honesty is carried into the research record as a per-item status.**

`docs/research/integrations/google.md`'s Verification Plan had eleven items and said "none exist yet". It now
marks each item **WRITTEN** or **Not written**, names the one that is written, and states that six fixtures
exist and **none is a capture**. A plan where every line is unmarked reads as done; a plan where every line is
marked is auditable.

## Consequences

- **"We have fixtures" can no longer be read as "we have a recording".** The distinction is in the file, checked
  by a test, and repeated in the research record.
- **A real capture is a visible change.** Replacing a file means deleting `_not_a_capture`, which the sweep
  would accept only if the marker itself changed — so a capture arrives as an explicit edit a reviewer sees.
- **The mutual-exclusion constraint is now tested in both directions**, and the earlier false fixture is gone.
- **The fixtures prove the reader and not the record.** Six fixtures and nine tests now establish that this
  crate reads documents matching Google's published shapes. They establish **nothing** about what Google sends,
  because nothing was sent.

## Limits

- **No fixture is a capture, and no live call has been made.** No credential exists, no Cloud project was
  created, and no Google API was contacted. The opt-in live smoke test named in the Verification Plan is not
  written.
- **A hand-built fixture cannot reveal a constraint nobody documented.** The mutual-exclusion rule was found
  because Google states it in a field description; a constraint Google enforces but does not write down would
  still be invisible, and only a capture would find it.
- **The fixtures do not cover every operation the connector declares.** `gmail_messages_read` is covered by the
  `Message` resource, but the response *envelope* for a `format=metadata` request, the batch endpoint, and
  `history.list` — the operation Finding 2 most depends on — have no fixture at all.
- **The sweep checks provenance and not shape.** It asserts every fixture is JSON and declares itself; nothing
  validates a fixture against the tool's declared output schema, so a fixture could drift from the schema
  `ADR-0059` derives and only a reading would catch it.
- **`_shape_documented_at` is a URL a maintainer must re-check.** Nothing verifies the page still says what the
  fixture assumes, and nothing records *when* it was checked. A dated verification entry in the research record
  is the existing mechanism, and it is a convention rather than an assertion.
