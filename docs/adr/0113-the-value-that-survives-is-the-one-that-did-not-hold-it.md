# ADR-0113: The value that survives the `watch` is the one the renewal needs, and it was the one that did not hold it

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — closing the loop from a created channel to its renewal). Makes the
  channel path's decision chain reachable from the value that survives the `watch`.
- **Relates to:** `ADR-0107` (the identical omission one round earlier — `resourceId` read and dropped), `ADR-0106`
  (the three encodings of one expiry and the `renewal_decision` this feeds), `ADR-0112` (the create builder that
  makes the response this registration is built from obtainable), `ADR-0111` (the same "a value the decision
  needs" question for the Gmail lease), and `ADR-0092` (a value whose consumer is asserted in prose — here a
  *decision* whose only input the surviving record does not carry).

## Context

`ADR-0112` gave the connector a way to **create** a channel (`calendar_channel_watch`), and
`parse_channel_watch_response` reads the response's `expiration` into `ChannelWatchResponse::expires_at`. That
field's own doc says it is *"a hard boundary"* that *"can drive a renewal decision"*, and `renewal_decision`
takes **the expiry as its only input** — it exists to answer, of a channel, *"replace now, replace soon, or
nothing to do"*.

**The registration — the value that survives the `watch` — had no field for the expiry.** `ChannelRegistration`
is documented as *"the value that survives between the `watch` and the teardown"*, and it already carries the
channel id, the resource id, the account and the token. Two facts made the omission consequential rather than
untidy:

1. **`renewal_decision` was reachable only from a test.** Nothing that persisted across calls held a channel's
   expiry, so nothing in production could feed the decision — the *same* shape `ADR-0107` found for
   `resourceId` (*"a value read and then dropped"*) and `ADR-0092` found for the Gmail watch anchor. The
   decision's own doc describes a caller deciding *when* to renew; that caller had no value to decide from.
2. **The type's own doc counted the facts, and the count was wrong.** It said *"two of the four facts are the
   provider's and two are not"* — and it listed four. There were in fact **five** facts in play (three from the
   `watch` response: `id`, `resourceId`, `expiration`; two the connector's own: account, token), and the one the
   count had no room for was the expiry. **A doc that enumerates its inputs is a claim about the set; an
   omission in the enumeration is invisible because the arithmetic adds up.**

So the slice is not "add a field". It is: **the record that survives a call must carry every value a later
decision about that call needs, and the expiry is one the renewal has as its sole input.**

## Decision

**1. `ChannelRegistration` gains the provider's expiry.**

```rust
pub struct ChannelRegistration {
    pub channel_id: String,
    resource_id: String,
    pub account: AccountReference,
    token: Option<SecretValue>,
    expires_at: UtcTimestamp,      // new: the provider's reported expiry
}
```

It is a stored **fact**, not a decision: deliberately **not** a `ChannelLease` or a `ChannelRenewal`, because
those are *answers* to a question asked at an instant and storing one would pin it to the second it was
computed — the reason `ChannelWatchResponse` holds an instant and the decision is a function of it and `now`.

**2. `ChannelRegistration::from_watch_response` is the constructor a caller should reach for.**

A channel's two identifiers **and** its expiry all come from the one `watch` response; only the account and the
token are the connector's own. The plain `new` therefore requires a caller to have split those sources
correctly, and the defect the named constructor prevents is exactly **dropping the expiry** — the same
omission shape as `resourceId`, from the same call. It takes the response as a whole so the fact it carries
travels with the two beside it. It **refuses a blank `resourceId`**, because the response parser checks presence
and type but not usability, and a blank second stop identifier builds a registration that cannot end its own
channel.

**3. `ChannelRegistration::renewal(now)` is the bridge, and it delegates rather than reimplements.**

```rust
pub fn renewal(&self, now: UtcTimestamp) -> ChannelRenewal {
    renewal_decision(self.expires_at, now)
}
```

The bridge from the stored fact to the decision, and the one place the two halves meet. A test asserts
`registration.renewal(t) == renewal_decision(registration.expires_at(), t)` so the bridge is provably **not a
second opinion** — two implementations of one decision is the defect this repository records for every
duplicated rule.

**4. The `Debug` prints the expiry, because it is an instant rather than content.**

The hand-written `Debug` already redacts the token; the expiry names no secret and is what a diagnostic about a
stale or nearly-lapsed channel needs to show.

## Consequences

- **The renewal decision is reachable from production.** A caller holding a registration — the value that
  survives the `watch` — can ask whether to replace its channel without the response it was built from, which is
  the whole point of the registration surviving.
- **`ChannelWatchResponse`'s `expires_at` doc is no longer a claim with no consumer.** It said the value *"can
  drive a renewal decision"*; now the value that survives carries it and the decision is a method away rather
  than orphaned from its input.
- **The create→register→renew loop is closed at the far end.** `ADR-0112` built the create; this makes the
  response's expiry reach the decision, so the chain
  `calendar_channel_watch` → `ChannelWatchResponse` → `ChannelRegistration` → `renewal_decision` is expressible
  rather than three pieces joined only in tests.
- **Three guards falsified A-B-A**, all compiling: (1) `renewal()` ignoring the registration's expiry
  (`renewal_decision(now, now)`) → **detected**; (2) the blank-`resourceId` refusal removed → **detected**
  (`left: Ok(ChannelRegistration { … resource_id: "   " … })`, `right: Err(Missing { field: "resourceId" })`);
  (3) `from_watch_response` not carrying the expiry → **detected**.
- **The end-to-end fixture test now proves the surviving value**, not just the response in hand: it builds a
  registration from the recorded `watch` response and asserts the registration's own renewal answer equals the
  free function's against the same fixture.

## Alternatives considered

- **Store a `ChannelLease` on the registration instead of the instant.** Rejected: a lease is the answer to a
  question asked at one instant, and storing it would make the registration wrong a second later. The expiry is
  the fact the question is asked *about*; `renewal(now)` is the answer, computed on demand.
- **Recompute the expiry from the request's requested lifetime rather than read the response's.** Rejected: the
  guide says the actual value is *"determined either by your request or by any Google Calendar API internal
  limits or defaults (the more restrictive value is used)"*, so a recomputation would disagree with the provider
  exactly when Google shortened it — the case a renewal decision most needs to be right about. The provider's
  reported instant is the only honest figure, which is also why `calendar_exposure` reads a channel's own lease
  rather than a stated bound (`ADR-0106`).
- **Keep `new` as the only constructor and document the five facts.** Rejected: it leaves the expiry to be
  remembered by a caller that also has to split provider facts from connector facts, which is precisely how it
  was dropped once. A constructor that takes the response makes the split unnecessary for the three provider
  facts, and the plain `new` stays for tests and for a caller that genuinely holds loose values.
- **Add the expiry but no `renewal` method.** Rejected: the field alone would make the decision reachable *in
  principle* while leaving every caller to re-derive `renewal_decision(registration.expires_at(), now)` — and a
  caller that forgot to would have a stored channel it never replaces. The method names the question, which is
  the `storable`/`is_storable` convention `ADR-0108` establishes for an accessor that must be reached for
  deliberately.
- **Store the `ChannelLease::Alive`/`Lapsed` variant computed at registration time.** Rejected for the same
  reason as the first alternative: it is a decision, not a fact, and it would be stale immediately.

## Conditions that would justify revisiting

- **A renewal caller is built** (`P6`/a scheduled worker), at which point `renewal()` gains its first production
  caller and the `CHANNEL_REPLACE_LEAD_SECONDS` margin and this bridge are exercised together — the limit
  `ADR-0112` records as "no scheduler renews".
- **A registration is persisted**, at which point the `UtcTimestamp` serialization and the column that holds it
  become `jarvis-storage`'s concern and the field is confirmed round-trippable.
- **A channel's expiry is refreshed without recreating the channel**, which the guide says is impossible
  (*"there's no automatic way to renew"* — renewal is a **replacement**), so this would be a provider change.
