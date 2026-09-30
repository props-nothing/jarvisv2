# ADR-0097: A delivery names a mailbox, and nothing mapped it to an account

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector — attributing a push delivery to a connected account).
- **Relates to:** `ADR-0094` (a negative acknowledgement is charged to the subscription — the cost that decides
  every unroutable case here), `ADR-0091` (the mailbox address is a person's, so `PubsubNotification` redacts
  it), `ADR-0096` (the identity read whose `emailAddress` is the value matched here), and `P5-004`'s Finding 1,
  which establishes that the connector **cannot authenticate** either push mechanism.

## Context

Every piece of the Gmail push path existed in isolation. A delivery could be **decoded** (`parse_delivery`,
`PubsubNotification { email_address, history_id }`), its lease **read** (`watch::parse_watch_response`), its
answer **decided** (`pubsub::decide_acknowledgement`), and a stale cursor **classified**
(`client::advance_gmail_history`). And `gmail_profile_read` now supplies an account's verified identity.

What was missing is the join: **which of the connector's accounts does this delivery name?** A notification
says "a mailbox changed" and gives an address; the sync it triggers has to run through one account's stored
credential. Nothing connected the two, so a delivery could not be attributed and therefore could not be acted
on.

**And the value that must be joined on is untrusted.** `P5-004`'s Finding 1 establishes that neither Google
mechanism fits `WebhookSupport::Push` — Gmail's push is an **OIDC bearer JWT** and Calendar's is an **echoed
channel token over a zero-length body**, while `SignatureScheme` is HMAC-family only — so the connector has
**no way to verify a delivery came from Google at all**, and that remains an unresolved question. The
`emailAddress` in a notification is therefore a string supplied by whoever posts to the endpoint:

> "a payload that decodes to a JSON object with `emailAddress` and `historyId`"

So a routing decision that treated the address as **authority** would let a forged delivery choose which
account is read. That is the tension this ADR resolves, and it is why the module's doc states the trust
boundary rather than only implementing a lookup.

## Decision

1. **`DeliveryRoute` has four variants, not an `Option`.** `Exact(AccountReference)`,
   `Ambiguous { accounts }`, `CaseDiffers { accounts }`, `Unknown`. An `Option` has two states and the decision
   has four: a reader of `None` could not distinguish "not my account" from "looks like my account, spelled
   differently", and those call for different operator action.

2. **The comparison is byte-exact, and that is a security property rather than strictness.** The address is
   untrusted input from an unauthenticated endpoint, so a `starts_with`, `contains`, or trimmed comparison
   would let a delivery select an account it does not name. The tests assert six near-misses that each defeat a
   looser rule — a prefix, a superstring, a suffix-domain, a different local part, and leading/trailing
   whitespace.

3. **A case-only near-match is reported, never promoted.** Addresses are case-insensitive in practice, so
   `Person@example.invalid` is *probably* the same mailbox as a stored `person@example.invalid` — but Google
   publishes **no canonicalisation statement** for `emailAddress` in a push payload. If the two spellings were
   genuinely two accounts, applying the route would read the wrong mailbox. So `CaseDiffers` exists as a state
   a person resolves, and it carries a **count** rather than a reference.

4. **An ambiguous route selects nothing.** `Exact` carries a reference; `Ambiguous` and `CaseDiffers` carry
   counts, and `account()` returns `Some` for `Exact` alone. Picking the first, the oldest, or the most
   recently verified would be an arbitrary choice that decides which mailbox is read, and a wrong pick syncs one
   mailbox's changes under another account's identity — so no accessor can return an account from them.

5. **`route_delivery` takes `&[VerifiedAccount]`, not addresses.** The address and the reference must belong to
   **the same account**. A `&[(AccountReference, String)]` argument would let a caller pair one account's
   reference with another's address, routing a delivery to the wrong mailbox with nothing able to notice. The
   argument is the type that already binds the two.

6. **Every unroutable delivery is acknowledged, and `Retry` is unreachable from this module.** None of
   `Unknown`, `Ambiguous` or `CaseDiffers` is repaired by another attempt — the account set is a **local** fact,
   so an address that matches nothing today matches nothing on the next delivery either. `ADR-0094`'s finding
   makes that decisive: a negative acknowledgement triggers a **subscription-global** backoff of up to 60
   seconds that the subscriber cannot disable, so refusing an unroutable delivery would slow **every other
   mailbox on the subscription** for a message that can never become routable.
   `unroutable_acknowledgement()` returns `AbandonAndAcknowledge` for all three, and `None` for `Exact`.

7. **What bounds a forged delivery is stated, because it is not authentication.** Two properties, both
   load-bearing:
   - **The route selects a mailbox to *read*, never a credential to use.** The sync runs through the account's
     own stored token, so a forged delivery cannot reach a mailbox the connector was not already authorised to
     read. Its worst case is a **spurious sync of an account the attacker already knew about**.
   - **The notified `historyId` is not a position the sync trusts.** The read that follows is `history.list`
     from the connector's **stored** cursor, and `advance_gmail_history` refuses a marker that moves backwards.
     A forged id that is too *high* cannot skip changes, because `history.list` returns everything after the
     stored position regardless of what the notification named — so a forged id is not a way to make the
     connector **miss** mail, which is the failure that would matter.

   So the honest statement is: routing decides **which known account to read**, and authorization was settled
   when the account was connected. Nothing here returns a credential, and no field could hold one.

## Consequences

- **The push path is now end to end minus the transport**: decode → route → (sync) → advance → acknowledge.
  The join that was missing exists, and a delivery that names a connected mailbox has a defined next step.
- **Two guards were falsified A-B-A**, both compiling: promoting a case-differing match to an exact one (caught
  by **four** tests, because the mutant changes what every route returns for a stored account), and making an
  unroutable delivery `Retry` instead of `AbandonAndAcknowledge` (caught by the acknowledgement test).
- **A generalisation worth keeping:** *an identifier that arrives over an unauthenticated channel may select,
  but must not authorise.* The safe routing rule is not "trust the field" or "refuse the field" but "use the
  field to choose among things you already authorised, and make sure a wrong choice costs a read rather than a
  grant". Here that is exactly what the two bounding properties do.
- **And a second:** *a near-match is a state, not a match.* Case-only equality is *evidence* about identity and
  not identity itself, and the difference is a person's call when the provider publishes no rule — the same
  shape as `ADR-0067`'s self-correcting `404`, except that here no cheap action is self-correcting, so the
  decision waits.
- **A limit remains:** no delivery has been received and no account has been connected, so the routing runs on
  types the crate owns rather than on observed data. And the **authentication gap is untouched**: this ADR
  makes a forged delivery's consequence small and bounded, and does **not** make forging impossible — that is
  Unresolved Question 1, and it needs either a contract change or an OIDC/JWKS verifier that `P5-001` kept out
  of a pure path on purpose.

## Alternatives considered

- **Match case-insensitively and treat it as a match.** Rejected: it is probably right and unverifiable, and
  the cost of being wrong is reading one mailbox's changes under another's identity. Reporting the near-match
  costs a person's decision and no correctness.
- **Promote a case-only match when exactly one account matches, and report only when several do.** Rejected for
  the same reason at a smaller scale: a **single** account differing only in case is still the ambiguous case,
  because the question is whether the *provider* means the same mailbox — and `Ambiguous` here counts local
  account rows, not the provider's intent.
- **Take the first match when several accounts carry an address.** Rejected: it converts a detected data
  problem (a reconnect that did not retire the old row) into a silent, arbitrary routing decision.
- **Return `None` for anything that is not exact, and log the rest.** Rejected: it erases the distinction
  between "not my account" and "my account, spelled differently" — the "two situations, one rendering" defect
  this repository keeps recording, and the one that leaves an operator with nothing to act on.
- **Refuse an unroutable delivery so it is retried later.** Rejected: the account set is local and the address
  will match on no future attempt, so the retry never succeeds — while each negative acknowledgement costs
  **every other account on the subscription** up to 60 seconds of backoff (`ADR-0094`).
- **Look the address up in a store rather than taking accounts as an argument.** Rejected: this crate has no
  store, and taking the accounts makes the routing a **pure function** of its arguments — the property every
  decision in this module family is built on, and what makes the four routes testable without a database.
- **Widen `SignatureScheme` to cover OIDC so the delivery can be authenticated first.** Rejected *for this
  slice*, not as wrong: it is Unresolved Question 1, it adds a JWKS network dependency to a path `P5-001` kept
  pure, and it is a **contract** change rather than a routing one. Routing a delivery correctly is worth doing
  even when authentication arrives, because the address still has to be mapped once it is trusted.
- **Ignore the address and use `deliveryAttempt`/`messageId` scoping instead.** Rejected: neither names a
  mailbox, and the delivery carries no other field that does — the address is the only routing key the provider
  sends.

## Conditions that would justify revisiting

- Push authentication is added (Unresolved Question 1), at which point the address becomes trusted input and
  the case-only near-match could be revisited — with the provider's canonicalisation rule, which is what is
  missing today rather than the willingness to apply one.
- A connect-time flow is built that retires a superseded account row, which would make `Ambiguous` an invariant
  violation rather than a reachable state, and a test could then assert the invariant instead of the route.
- A second provider's push path is added (`P5-006`, Microsoft Graph change notifications), at which point the
  question of whether routing belongs in a shared module or stays Google-specific becomes live. Graph names the
  `subscriptionId` rather than an address, so the key differs and the comparison rules would too.
- A local account store gains a **canonical** form for provider addresses — a stored lowercase copy checked by
  the provider — at which point `CaseDiffers` could collapse into `Exact` without an assumption, because the
  canonicalisation would be the connector's own recorded rule rather than a guess about the provider's.
