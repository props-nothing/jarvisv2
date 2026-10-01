# ADR-0112: A prescribed call with no builder, and a body that made a request's own doc false

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — creating a Calendar notification channel). Closes the create
  direction of the channel path `ADR-0100`–`ADR-0111` built at its far end.
- **Relates to:** `ADR-0106` (the channel lease this creation's response produces and the `renewal_decision`
  that prescribes the call), `ADR-0107` (the `channels.stop` this is the other half of, and the `resourceId`
  the response returns), `ADR-0093` (the `JsonRequest` type this makes the redaction body-dependent for),
  `ADR-0091` (four types that held a sensitive field and printed it — the defect a third body would have
  repeated), `ADR-0092` (a value whose consumer is asserted in prose — here a *prescribed call* with no
  builder), and `ADR-0066` (the redundant-guard rule, applied to why the token bound is not re-checked).

## Context

`channels.stop` was built and its arity recorded (`ADR-0106`, `ADR-0107`), and `renewal_decision` prescribes
the remedy for a channel that has lapsed or is about to:

> "Currently, there's no automatic way to renew a notification channel. When a channel is close to its
> expiration, you must **replace it with a new one by calling the `watch` method**."

**No builder could issue that call.** `parse_channel_watch_response`, `ChannelLease`, `ChannelRenewal` and
`renewal_decision` were all built around a `watch` **response** that nothing could obtain, so the entire
lease/renewal chain was reachable only from a hand-built fixture. This is `ADR-0092`'s "a remedy with no
consumer" from a new direction: not a decision without a caller, but a **prescribed call with no builder** —
the connector could `stop` a channel and had no way to `create` one.

Three facts made the gap worth closing rather than recording:

1. **The create and stop bodies are two halves of one record.** A `ChannelRegistration` holds the channel id the
   connector chose and the `resourceId` the provider returned. `id` is sent on creation and **echoed** back
   (`X-Goog-Channel-ID`), `resourceId` is returned on creation and required by stop — so creation is what makes
   a registration assemblable from a real response (`ADR-0107`'s wiring gap, at the other end).
2. **The request body carries two credential-adjacent fields**, and the crate's `JsonRequest` had a **derived**
   `Debug`.
3. **The fields are validated against rules that live on three different pages**, and two of them are
   provider-side facts that are not checkable locally.

## Decision

**1. A `calendar_channel_watch` builder, with the required trio spelled and the optional token carried.**

```rust
pub fn calendar_channel_watch(
    calendar_id: &str,
    channel_id: &str,
    address: &str,
    token: Option<&SecretValue>,
) -> Result<JsonRequest, RequestError>;
```

Bodies produced: exactly `{id, type, address}` or `{id, type, address, token}`. `type` is always
`WEBHOOK_CHANNEL_TYPE` (the guide's `web_hook`) and is **not a parameter** — Google describes no other delivery
mechanism, so a caller has no valid alternative. **No expiry parameter** is offered: the request-side expiry is
governed by `params.ttl` on some surfaces and by internal limits on others (*"determined either by your request
or by any Google Calendar API internal limits or defaults"*), the figure JARVIS acts on is the **response's**,
and offering one would be a second place a channel's life is decided.

**2. `JsonRequest`'s redaction becomes body-dependent, and the choice is forced.**

The type's `rendered_body` doc said *"there is no credential here"* and it derived `Debug`. Both were true of
its two bodies (a topic name with label ids; a channel id with a resource id) and **false of the third** — a
channel **creation** body carries the webhook `address` and the channel `token` (the anti-spoofing control
`verify_channel_token` compares against). So:

- `Debug` is hand-written and prints a sensitive body as `[REDACTED]` plus its length.
- The field is set by one of **two private constructors**, `renderable` or `sensitive`. There is no public
  constructor, so a fourth builder must **name** which kind of body it produces and cannot silently print a
  secret — the "unrepresentable rather than checked" shape this module already uses for a credential in a URL.
- `rendered_body()` still returns the real bytes whatever the body holds, like `FormRequest::rendered_body`:
  the transport needs them, so the accessor cannot redact and the `Debug` is what keeps a secret out of a log.

**3. The `address` is validated for what a string can prove, and the certificate rule is recorded as a limit.**

The guide requires HTTPS *and* a valid (non-self-signed, non-revoked, subject-matching) certificate at the
receiving host. `webhook_address` enforces an absolute `https://` URL with a non-empty host, no control
characters, and a [`MAX_WEBHOOK_ADDRESS_CHARS`](crate::google::request) bound. **The certificate is not
checkable here** — it is a fact about a TLS handshake a request value with no socket cannot perform — so a
callback with a bad certificate passes the builder and fails at **delivery** time, which is recorded rather
than pretended. `http://` is refused rather than downgraded, and the scheme comparison is case-insensitive
because a URI scheme is.

**4. The channel id is bounded by Google's 64, not the module's generic 256.**

The push guide gives *"Maximum length: 64 characters"* for the `id`. A 65-character id passes the generic
`resource_id` bound and is refused by the provider, so the builder checks Google's figure **on top of** the
shared validator (still naming `channel_id`), and the boundary is exercised at 64 (accepted) and 65 (refused)
so neither side is unreachable.

## Consequences

- **The channel path is closed at both ends.** A channel can be **created** (`calendar_channel_watch`) and
  **stopped** (`calendar_channel_stop`), and the two share the identity a `ChannelRegistration` holds: the id
  the connector sent (echoed) and the `resourceId` the response returned.
- **`JsonRequest`'s own doc is no longer false.** A type whose `Debug` printed whatever it held now redacts a
  body that holds a secret, and the two bodies that hold none still print in full — asserted, because a
  redact-everything `Debug` would satisfy the redaction test while destroying the diagnostic value of the other
  two.
- **The redundant-guard rule is applied rather than re-learned.** The token bound (256) is enforced by
  `SecretValue::new` already, so the builder does **not** re-check it: a second check of the same bound is the
  guard `ADR-0066` records as one that can never decide anything the first check did not.
- **Four guards falsified A-B-A**, all compiling: (1) the sensitive constructor setting `false` → the token
  appears in a `Debug` → **detected**; (2) `strip_https_scheme` accepting any non-empty scheme → `http://`
  accepted → **detected**; (3) the channel-id bound relented to `MAX_RESOURCE_ID_CHARS` → a 65-character id
  accepted → **detected**; (4) the **non**-sensitive constructor setting `true` → a printable body renders
  `[REDACTED]` → **detected**, which is the control that keeps the redaction honest.
- **The research record gained Finding 23** and a source-table row for the `events.watch` **request** body, so
  the field set, the three required properties and the two credential-adjacent fields are evidenced rather than
  recalled.

## Alternatives considered

- **Record the gap and leave the builder out (as `ADR-0106` did for the replacement).** Rejected here because
  the prescription is explicit (*"you must replace it with a new one by calling the `watch` method"*), the
  response parser and every lease type already exist to consume the result, and a remedy with no operation is
  the defect `ADR-0092` exists to remove. A recorded gap is right for a caller that does not exist; it is wrong
  for a **call** the documentation names and the connector's own decision function prescribes.
- **Give `JsonRequest` a `redact: bool` argument on a public constructor.** Rejected: a `bool` at a call site
  says nothing about which value it selects, and a public constructor lets a future builder pass `false` for a
  body that holds a secret with nothing to catch it. Two named private constructors make the choice **named**
  and **closed**.
- **Redact only the `token` field rather than the whole body.** Rejected: it leaves the decision to print the
  `address` to the `Debug`, and the body is short — redacting all of it costs nothing while removing the
  question entirely. The length is kept because it is not the value.
- **Enforce the HTTPS certificate rule.** Rejected, and recorded as a limit: a valid chain is observable only
  through a TLS handshake this crate cannot perform, so a "check" would either be a no-op or an unverifiable
  claim. The provider's delivery failure is the real signal, and the guide's own *"not 100% reliable"* caveat
  already tells a caller to reconcile.
- **Offer `params.ttl` as a parameter.** Rejected: JARVIS replaces a channel on its **own** lease figure
  (`CHANNEL_REPLACE_LEAD_SECONDS`) and reads the actual expiry from the response, so a request-side ttl would be
  a second place a channel's life is decided — and the two would silently disagree when Google applies *"the more
  restrictive value"*.
- **Accept `type` as a parameter defaulting to `web_hook`.** Rejected: it is a field with one usable value, and
  a parameterised field with one value is the "a value nothing is decided by" shape (`ADR-0093`'s reasoning for
  renaming a type rather than generalising it).

## Conditions that would justify revisiting

- **A live smoke test runs**, at which point the certificate rule and reachability are exercised for real, and
  the "passes the builder, fails at delivery" limit is proven or disproven rather than recorded.
- **A second Google `watch` resource is added** (`acl.watch`, `calendarList.watch`, `settings.watch`), at which
  point the shared body shape may want a builder per resource path — the reference's *"there's only one `stop`
  method"* symmetry means one *stop* already serves all of them, and the create side would want an argument for
  the collection rather than a builder per resource.
- **A caller that renews on a schedule is built**, at which point the `CHANNEL_REPLACE_LEAD_SECONDS` margin and
  this builder are exercised together and the request-side expiry question may be revisited if Google's
  *"more restrictive value"* behaviour makes a stated ttl necessary.
- **`JsonRequest` gains a fifth body**, at which point the two-constructor rule is the test of whether the
  closure held: a builder that cannot be built without naming `renderable` or `sensitive` is the property this
  design rests on.
