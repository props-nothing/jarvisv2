# ADR-0098: One address, one account — and the opposite direction from routing

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — connecting an account, and resuming a sync).
- **Relates to:** `ADR-0096` (the identity read this flow consumes, and which named this flow as unbuilt),
  `ADR-0097` (the router whose `Ambiguous` state this step prevents or creates), `ADR-0091` (the address is a
  person's, which the *third* finding below is about), `ADR-0067` (a cause the provider publishes no code for),
  and `crate::cursor::SyncCursorKind::Start`, whose "a full sync is a decision" is what `ResumePoint` reports.

## Context

Two adjacent slices each named a step that did not exist. `ADR-0096` gave the connector an operation reading a
mailbox's **verified identity** — `gmail_profile_read`, returning `{ emailAddress, historyId }` — and recorded
that "the connect-time identity flow (call `gmail_profile_read`, build `VerifiedAccount`, store
`AccountReference`) does not exist, so the requirement is now *satisfiable* rather than *satisfied*". `ADR-0097`
gave the push path a **router** attributing a delivery to one of the connector's accounts, and named
`DeliveryRoute::Ambiguous` — several account rows carrying one address — as reachable, caused by "a reconnect
[that] mints a new `AccountReference` without retiring the old row".

Those are the two ends of one step. The profile read produces an identity; the router consumes stored
identities; **nothing turned one into the other**, and the state the router cannot act on is created exactly
there. So this slice is where `Ambiguous` is **prevented or created**, and that is why the rule it carries is a
**refusal** rather than a deduplication.

What a duplicate costs is the argument, and it is not tidiness:

- **Routing stops being decidable.** Once two accounts carry one address, `route_delivery` answers `Ambiguous`,
  and that route may not be applied without a person — so every notification for that mailbox stops being acted
  on until someone resolves it. A connector that could still read the mailbox on demand would silently stop
  receiving its **changes**, which is precisely the degradation a push path exists to prevent.
- **And the duplicate is invisible in the meantime.** Two accounts for one mailbox look exactly like two
  mailboxes: two cursors, two sync schedules, and a quota budget paid twice for one mailbox's traffic.

## Decision

1. **`establish_account` refuses a second account for an address another account already holds**, naming the
   **holder** in the refusal so an operator can act on it. "This address is taken" without saying by what leaves
   a person searching, and the holder is JARVIS's own opaque reference rather than the address.

2. **The duplicate comparison ignores ASCII case, and that is deliberately the opposite direction from the
   router.** `ADR-0097` refuses to **act** on a case-only near-match, because Google publishes no
   canonicalisation rule for `emailAddress` and promoting the near-match could read the wrong mailbox. This
   refuses to **create** one, for the same underlying uncertainty: if the two spellings are one mailbox,
   allowing both mints the duplicate above; and if they are genuinely two mailboxes, the cost of refusing is
   that **a person is asked**, which is recoverable, where a wrong automatic action is not.

   **So the two conservative choices point in opposite directions and they are consistent: neither acts on an
   uncertain case-match.** One test asserts both halves against a single pair of spellings, so a later change
   loosening either would appear as a contradiction rather than as a silent policy drift.

3. **An identity this platform cannot store is its own refusal (`IdentityUnusable`), not the duplicate one.**
   The two have different **subjects** — one is about the account set, one about the identity — and different
   remedies: a duplicate is fixed by retiring an account, while an unusable identifier points at a provider
   response. Reporting the second as the first would send a person looking for a duplicate that does not exist.
   `holder()` therefore returns `Option`, `None` for this variant, rather than fabricating a holder to make the
   signature uniform.

4. **`display_name` is `None`, because `users.getProfile` returns no display name.** Its reference gives
   `emailAddress`, `messagesTotal`, `threadsTotal` and `historyId`. Passing the address as a display name would
   invent a provider statement, which is the field `VerifiedAccount` exists to keep honest.

5. **`resume_from(cursor)` reports where a sync begins, and carries the cursor's *kind* with its position.**
   The kinds differ in exactly the way that matters here: a `MonotonicMarker` (Gmail `historyId`) has detectable
   staleness and a defined recovery, while an `OpaqueToken` (Calendar `nextSyncToken`) may not be validated at
   all and only the provider's refusal is authoritative — so a caller must not have to remember which API it is
   looking at.

6. **A `Start` cursor is reported as `FullSync`, not as an error, and the two causes behind it are not
   separated.** `SyncCursorKind::Start`'s own doc says it means *"the connector has never synced, or its
   position was discarded"* — two causes, one value — and a full sync "is a decision with consequences (cost,
   time, possibly a rate-limit budget), and an absent value would make it the default a caller stumbles into".
   A caller wanting the reason already has it (it is the component that just received `HistoryPruned` or
   `TokenInvalidated`), so this reports the **action** rather than inventing a discrimination the input cannot
   support — the restraint `ADR-0067` records for a `404` whose cause Google publishes no code for.

7. **`resume_from` does not re-check that a `Start` cursor carries no token.** `SyncCursor::new` already refuses
   both impossible combinations, so re-asserting them here would be a refusal that can never fire — the defect
   `ADR-0066` and `P5-003` each record.

## Consequences

- **The two ends are now joined and the router's `Ambiguous` has a prevention.** The test walks the
  counterfactual as well: had the duplicate been created, the same notification would be `Ambiguous`, and the
  test constructs that duplicate directly so the consequence is **demonstrated rather than described**.
- **⚠ A THIRD FINDING, AND IT WAS FOUND BY A FAILING TEST RATHER THAN BY REVIEW.** The `IdentityUnusable`
  refusal first carried `error.to_string()`, propagating `ConnectorError::Identifier`'s rendering — whose own
  `Display` is `"the connector identifier `{value}` is unusable: {reason}"`, **interpolating the value it
  rejected**. So an unstoreable address would have been printed in full by the refusal that says an identity
  could not be stored, **while `VerifiedAccount`'s `Debug` redacts that same value** (`ADR-0091`). The test
  asserting the refusal does not contain the address is what caught it; the fix was to carry the error's
  **`&'static str` `reason` field** instead of its rendering, which makes the leak **unrepresentable** — a
  `&'static str` has nowhere to put a runtime value.
  - **The generalisation:** *redaction applied to a value in one place is not redaction applied to it in
    another* — and the second place is often an **error path**, because a `Display` that exists to be helpful
    to a developer is exactly where a value gets interpolated. The move that fixes it is to carry the **reason**
    rather than the rendering, which is structural rather than a rule about not printing.
- **Two guards were falsified A-B-A**, both compiling: dropping the case-insensitivity from the duplicate check,
  and reporting a cursor that *has* a position as needing a full sync.
- **A limit remains:** nothing calls `establish_account` or `resume_from`. There is no account store, no
  executor and no sync loop in this crate, so both are decisions with tests rather than enforced behaviour —
  the same "convention with tests, not a mechanism" limit `ADR-0091` and `ADR-0095` each record. The two causes
  of `FullSync` also stay unseparated, deliberately (decision 6), which means a caller that wants the reason
  must hold it itself.

## Alternatives considered

- **Allow a duplicate and let the router's `Ambiguous` handle it.** Rejected: `Ambiguous` is unactionable
  without a person, so allowing the duplicate converts a detectable connect-time mistake into a **silent**
  stop in change notifications for that mailbox, plus a doubled quota budget — while the duplicate itself looks
  like two mailboxes.
- **Deduplicate silently — reuse the existing account for a reconnect.** Rejected: a reconnect may be a
  **different grant** (new scopes, a new consent, a rotated refresh token) and reusing the row would discard
  that, or overwrite a scope set the user deliberately narrowed. Whether a reconnect should reuse or replace is
  a **person's** decision and the refusal surfaces it.
- **Match addresses case-sensitively, to match the router exactly.** Rejected: routing must not *act* on an
  uncertain match, which is a different question from whether a duplicate should be *created*. Creating one is
  the irreversible direction here, so the conservative choice is the opposite one.
- **Lowercase-normalise the stored address and compare canonically.** Rejected *for now*, not as wrong: it
  would make the comparison exact and remove the near-match entirely, but it needs a **recorded rule** for what
  canonical form means, and Google publishes none. It would also change what `provider_account_id` holds, which
  is the value a request is addressed with — so it is a storage decision rather than a connect-time one. Named
  in `ADR-0097`'s revisit conditions as the way `CaseDiffers` could eventually collapse.
- **Return the address in the `IdentityUnusable` refusal so the operator can see what failed.** Rejected: the
  address is a person's mailbox, the *reason* is what an operator acts on, and the first version of this
  variant did expose it — which is the finding above. A diagnostic prints the bound, not the value.
- **Have `establish_account` take a `&[(AccountReference, String)]` of addresses.** Rejected for the reason
  `route_delivery` does not (`ADR-0097`): the address and the reference must belong to the same account, and a
  paired list would let a caller report the **wrong holder**, which is worse than no report because it sends an
  operator to retire the wrong account.
- **Have `resume_from` return the `SyncCursor` itself.** Rejected: it would leave the caller to re-derive
  whether a full sync is needed and to remember which kind of token it holds, which is the interpretation this
  function exists to perform once.
- **Refuse a `Start` cursor as "never synced" and require the caller to decide.** Rejected: that is the state
  `SyncCursorKind::Start` was introduced to make representable *as a decision*, and its doc says an absent
  value would make a full sync "the default a caller stumbles into". Reporting it as `FullSync` is what keeps
  the cost visible.

## Conditions that would justify revisiting

- An account store is built, at which point the duplicate check becomes a uniqueness constraint rather than a
  scan of a slice, and the rule can be enforced by the store as well as by this function.
- A connect-time flow is specified end to end — read the profile, establish the account, seed the cursor from
  the profile's `historyId`, call `users.watch` — which is where `establish_account` and `resume_from` become
  *used* rather than *available*.
- A reconnect policy is decided (reuse the reference, or mint a new one and retire the old), which would make
  `Ambiguous` an invariant violation rather than a reachable state and let the refusal become an assertion.
- Google publishes a canonicalisation rule for `emailAddress`, or a local canonical form is recorded, at which
  point decision 2's case-insensitive comparison can become an exact one and the near-match disappears from
  both this module and `ADR-0097`'s.
- The profile read gains a display name or another identity field, at which point decision 4's `None` should be
  revisited — but only against the reference, since the current response carries no such field.
