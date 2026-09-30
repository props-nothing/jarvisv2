# ADR-0082: One classifier for two APIs answered for the less informative one

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0058` (a provider's decisions belong in the crate with no socket — which introduced the
  Gmail-typed vocabulary), `ADR-0081` (Calendar's 410 needs its reason), `ADR-0080` (a limit and a
  recommendation are two facts), `P5-005`.

## Context

This connector declares **four** operations, three against Gmail and one against Calendar, and they share one
classifier:

```rust
pub enum GmailErrorReason { /* … */ }

pub fn classify(
    status: u16,
    reason: GmailErrorReason,
    …
) -> RetryDecision
```

Two problems, both invisible while only Gmail had tests.

**1. The vocabulary was named for one API and used for both.** `operations::refusal` parses **every** error
body — Gmail's and Calendar's — into `GmailErrorReason`, and that name appeared in the public API of a type
that Calendar responses flow through. Naming is normally cosmetic; here it was load-bearing, because it made
the *shared* nature of the type invisible, and a reader deciding whether an arm applies to Calendar would have
to notice that the type is used for both.

**2. The two APIs publish different status sets, and the classifier could not tell them apart.** Google
documents them on **separate** pages:

- The **Gmail** page's status summary lists `200`, `400`, `401`, `403`, `404`, `429` and the `5xx` family. It
  has **no `410` subsection at all**.
- The **Calendar** page documents `410 Gone` in detail, as a dead sync token whose remedy is *"wipe the store
  and re-sync"* — which `ADR-0081` then built a reason vocabulary around.

A classifier with no API parameter must answer for the **less** informative case, and it did: a Calendar `410`
fell through to the catch-all and reached a caller as

```
the provider answered 410 (unknown)
```

`Unknown`'s guidance is `Reconcile` — *"establish what happened before doing anything else"* — so a connector
that had just been told **"the sync token is no longer valid, a full sync is required"** reported that it did
not know what happened. The provider had named the cause; the classifier discarded it.

The same conflation ran the other way too, and that one is subtler: the shared `400 | 404` arm refuses with
`DoNotRetry` for both APIs, while Calendar's error page suggests **"use exponential backoff"** for a `404`.
Neither reading was recorded, so a reader comparing the code with the Calendar page would find a discrepancy
with no explanation.

## Decision

The classifier takes the API, and the shared vocabulary is renamed for what it is.

```rust
pub enum GoogleApi { Gmail, Calendar }

pub fn classify(
    api: GoogleApi,
    status: u16,
    reason: GoogleErrorReason,
    retry_after: Option<RetryAfter>,
    provider_request_id: Option<ProviderRequestId>,
) -> RetryDecision {
    let (class, guidance) = match (api, status) {
        (GoogleApi::Calendar, 400 | 404 | 410) | (_, 400 | 404) => {
            (RetryClass::Permanent, RetryGuidance::DoNotRetry)
        }
        (_, 401) => (RetryClass::Authentication, RetryGuidance::Reauthenticate),
        (_, 403) => match reason { /* unchanged, shared */ },
        (_, 429) => (/* unchanged, shared */),
        (_, 500 | 502 | 503 | 504) => (/* unchanged, shared */),
        (_, _) => (RetryClass::Unknown, RetryGuidance::Reconcile),
    };
    …
}
```

The two routes to `Permanent`/`DoNotRetry` are written as **one arm with alternation** rather than two arms
with identical bodies, because `clippy::match_same_arms` (pedantic, and denied in this workspace) refuses the
latter — and the lint is right: two arms that cannot drift are one arm. The distinction that matters is between
**the two routes**, and that is carried by the comment and the tests, not by the arm count. Note also that the
`(GoogleApi::Calendar, 400 | 404 | 410)` half adds nothing for `400`/`404` — they are already matched by
`(_, 400 | 404)` — and it is written out only so the Calendar route is legible at a glance.

`GmailErrorReason`/`GmailErrorBody`/`GmailErrorObject`/`GmailErrorEntry` became
`GoogleErrorReason`/`GoogleErrorBody`/`GoogleErrorObject`/`GoogleErrorEntry`.

Five properties are deliberate:

1. **The API is an input, not a second table.** Two tables would duplicate every arm the APIs *do* share —
   `401`, the `5xx` family, and `403`'s throttling reasons, which Calendar's page explicitly confirms behave the
   same ("`rateLimitExceeded` errors can return either `403` or `429` error codes—currently they are
   functionally similar"). A duplicated table drifts, and this phase has found that defect repeatedly. So there
   is one table and the API is a parameter, exactly as `status` and `reason` are.

2. **A Calendar `410` is `Permanent`/`DoNotRetry`, and that does not contradict the resync remedy.** A resync is
   not a *retry of this request* — the same request with the same token cannot succeed. The caller's next action
   is a **different** request (a fresh full sync), so refusing the retry is right and the distinction is
   spelled out in the arm rather than left to inference. A reader who took `DoNotRetry` to mean "give up on the
   sync" would be making the mistake the comment exists to prevent.

3. **The `400 | 404` divergence is recorded rather than silently resolved.** Calendar's page suggests backoff
   for a `404`; Gmail's states no action; the crate keeps `DoNotRetry` for both. The reasoning is that
   Calendar's own two documented causes are "the requested resource … has never existed" and "accessing a
   calendar that the user can not access" — and neither is repaired by sending the identical request again, so
   a retry fails identically until its budget runs out. The divergence is asserted in a test that **names which
   document wins**, so a future reader "fixing" the arm to match Calendar's sentence fails a test that explains
   the decision (`ADR-0081`'s "assert both sides" technique applied to a documented disagreement).

4. **The API is derived from the operation name, not passed alongside it.** `api_of(segment)` reads the
   `gmail_`/`calendar_` prefix the manifest's operation ids already use, so a caller cannot pair
   `calendar_events_read` with `GoogleApi::Gmail` — the pairing that would reintroduce the defect. Deriving it
   makes the wrong pair unrepresentable rather than merely discouraged, which is the move this crate makes
   everywhere else.

5. **The rename is part of the fix, not housekeeping.** A type named `GmailErrorReason` that Calendar responses
   are parsed into is a false statement about the code's scope, and it is exactly the class of defect
   `ADR-0074` records (a name asserting something the code does not do). Four types were misnamed, and all four
   now say Google.

## Consequences

- A Calendar `410` reaches a caller as a definite verdict with a documented remedy, instead of asking it to
  establish what the provider had already stated.
- The shared arms are asserted to be **API-independent by test**, so a future per-API fork has to be deliberate
  — and the `410` arm is asserted to differ, so the one genuine divergence cannot be flattened by accident.
- The record gains the **Gmail errors page's** status list as the evidence that Gmail documents no `410`, and a
  note that its `404` summary states no action where Calendar's suggests backoff.
- **Two mutants were falsified A-B-A**, both compiling:
  - removing the Calendar `410` arm so it falls to the catch-all (the original defect: detected);
  - making the `410` arm match **both** APIs by dropping the API from the pattern (the "one classifier for two
    APIs" defect in miniature: detected by the same test, which asserts the two APIs differ).
- **A limit remains:** the per-API behaviour is only testable where a response exists, and no request has been
  sent to either API — so the classification rests on the two pages, not on observed behaviour. And
  `GoogleApi` has exactly two variants because the connector declares exactly two APIs; a third (Drive, Tasks)
  would need its own arm set, and the enum is the place that decision becomes visible.

## Alternatives considered

- **Two classifiers, one per API.** Rejected: it duplicates the arms the APIs share, and a duplicated table
  drifts — the defect this phase has found in `client::classify`'s own history (`ADR-0074`, `ADR-0075`).
- **Treat a `410` as known for both APIs.** Rejected: Gmail documents no such status, so claiming a definite
  meaning for it would be inventing provider behaviour — the failure `ADR-0080` records for a figure with no
  source.
- **Leave the status unclassified for both and let the cursor path handle `410`.** Rejected: the cursor path
  *does* handle it (`calendar_signal`), but the **refusal path** a caller reads is the one an operator sees, and
  it was telling them the cause was unknown.
- **Change the `404` arm to match Calendar's backoff suggestion.** Rejected: backoff does not repair a
  nonexistent resource or an inaccessible calendar, and Gmail's page suggests nothing. The divergence is
  recorded and asserted instead.
- **Rename only the reason enum.** Rejected: the three body types are parsed from both APIs' responses, so the
  same false scope applies to each; leaving three of four misnamed would make the remaining name look
  authoritative.

## Conditions that would justify revisiting

- Google publishes a `410` for Gmail, which would make the arm shared and the parameter unnecessary for it.
- A third Google API is added to the connector, which would grow `GoogleApi` and need each shared arm
  re-checked against that API's page.
- A `404` on Calendar is observed to succeed on a retry, which would falsify the reasoning behind keeping
  `DoNotRetry` and would move the divergence to a per-API arm.
- The live smoke test runs, which would let the classification be checked against real responses rather than
  against two documentation pages.
