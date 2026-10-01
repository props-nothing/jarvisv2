# ADR-0099: A delivery can be authenticated without covering the body

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — the webhook authentication contract, widened to cover both push
  mechanisms). Resolves **Unresolved Question 1** of `docs/research/integrations/google.md`.
- **Relates to:** `ADR-0054` (the webhook contract this widens), `ADR-0057` (a declaration that cannot be made
  honestly is a missing variant, not a default value), `ADR-0096` (a requirement whose evidence is the operation
  that returns it — the same "check the thing exists" move, applied to a scheme), `ADR-0087` (a lease that lapses
  silently), `ADR-0091` (a value redacted in one place and printed in another), and `crate::webhook`'s own
  "everything checks before parsing" order.

## Context

`ADR-0054` defined `SignatureScheme { algorithm, header, encoding }` with `SignatureAlgorithm` an **HMAC-family
plus Ed25519** set, and built `WebhookDelivery`/`SingleHeader` semantics around verifying a MAC over the **raw
body**. `P5-004` then read Google's two push-authentication pages and recorded, as **Finding 1**, that neither
Google mechanism is such a signature:

- **Gmail** (via Cloud Pub/Sub) authenticates with an **RS256 JWT in the `Authorization` header**. Verification
  is signature validation against Google's rotating certificates plus an `email`/`aud` claim match; the
  **request body is not signed at all**.
- **Calendar** (notification channels) has **no signature**: `X-Goog-Channel-Token` is *"an arbitrary
  client-set string, echoed back"*, presented as the control that each delivery "is for a channel that your
  application created". The delivery has a **zero-length body**, so there is nothing to sign even in principle.

`SignatureAlgorithm` had no variant for either, so `GoogleConnector` could not declare
`WebhookSupport::Push` **truthfully** and declared `Polling { interval: Unknown }` instead. Finding 1 was
recorded as **blocking** for the push half of `P5-005` and for `P5-010`'s "webhook signature/replay tests" — and
`ADR-0097`'s routing record then stated the same gap as **untouched**.

**The gap was named "a webhook authenticated by an OIDC bearer JWT or an echoed channel token", and the name is
the finding's own clue: neither covers the body, and both still authenticate a delivery.** The contract had only
one axis — "which MAC over the body" — and used it to answer a second question, "what authenticates this
delivery", for which it is the wrong axis. A provider whose authentication is a header token was therefore
unrepresentable, and the safe fallback (`None`) said *nothing authenticates the delivery*, which is false.

## Decision

1. **`SignatureAlgorithm` gains `OidcIdToken` and `EchoedChannelToken`.** They name the two header-token
   mechanisms exactly: an OIDC ID token presented in a header (conventionally `Authorization: Bearer …`), and a
   shared value the connector itself chose and the provider echoes back. A closed set stays closed: a new
   provider scheme is a new variant, and the two named cases are the two `security.md`'s webhook row does not
   already cover.

2. **A second axis, `covers_the_body`, names the property that was being asked for all along.**
   `HmacSha256`/`HmacSha1`/`Ed25519` cover the raw bytes; `OidcIdToken`/`EchoedChannelToken`/`None` do not.
   `is_body_independent()` is its negation, named so the module's "every check runs before parsing" holds *for
   the right reason*: a body signature must be checked against the exact bytes that arrived, and a token has no
   bytes to read — so "before parsing" is not a constraint on it but a consequence of what it authenticates.

   **`covers_the_body` is deliberately not `is_keyed_mac`.** `Ed25519` covers the body with a public key
   (`is_keyed_mac == false`, `covers_the_body == true`) and an echoed channel token needs a secret but covers no
   body (`is_keyed_mac == false`, `covers_the_body == false`) — so the two questions are provably distinct, and
   a caller asks the one it depends on.

3. **A body-independent authenticator may not claim a signature encoding.** `encoding` describes how a
   signature's **bytes** are presented; a token presents an opaque header value, so `SignatureScheme::new`
   requires `SignatureEncoding::Raw` for it and returns a new `SignatureError::Encoding` otherwise. This is
   `ADR-0057`'s rule once more — a field that cannot take an honest value for a variant is **refused** rather
   than defaulted — and it is a distinct error from `Header` because the remedy differs: a bad header name is a
   typo, while an encoding on a token is a **misunderstanding** of the mechanism that a reviewer must correct.

4. **The manifest's push guard is unchanged and still refuses `None`** (`!scheme.authenticates()`). The
   two new variants pass it, because they *do* authenticate a delivery; only `None` — nothing authenticates
   this — is refused. So the "webhook spoof with its control removed" refusal keeps its exact meaning, and it is
   now provably **not** a synonym for "not an HMAC", which the acceptance test asserts by round-tripping every
   authenticator.

5. **The Google connector keeps `Polling`, and for a corrected reason.** One connector holds **one**
   `WebhookSupport` value and Google has **two** push mechanisms with different headers *and* different bindings
   (an OIDC JWT in `Authorization` with an account-in-body binding; a channel token in `X-Goog-Channel-Token`
   with an account-in-header binding) — so declaring one would be as incomplete as declaring neither. And
   **neither verifier is built**: the JWT needs JWKS fetching plus `aud`/`iss`/`exp` checking, and the channel
   token needs a stored value to compare. Naming an authenticator is not verifying one, so declaring `Push`
   would also demand the webhook signature/replay readiness items nothing can yet satisfy. The connector's
   prose that said "neither mechanism *fits*" is corrected — the mechanisms now fit; it is cardinality and an
   absent verifier that keep the declaration at `Polling`.

## Consequences

- **Finding 1's contract half is closed.** `SignatureScheme::new(OidcIdToken, "authorization", Raw)` and
  `SignatureScheme::new(EchoedChannelToken, "x-goog-channel-token", Raw)` are constructible, so a Gmail or
  Calendar push endpoint can now be *declared*; a pinned test asserts both (and a manifest-level test proves the
  guard accepts them). The connector still declares `Polling`, and for the reasons in decision 5.
- **The axis existed implicitly and was never named, which is why the gap survived two rounds.** `ADR-0054`'s
  prose argued that "a keyed MAC and an asymmetric signature both cover the body, but only the MAC requires the
  verifier to hold a secret" — i.e. it *knew* "covers the body" was one property and "needs a secret" another,
  and shipped accessors for only the second. **Generalisation: when a type answers two questions with one
  accessor, the unasked question is the one a new provider will need.** The fix is the missing accessor, which is
  why decision 2 is the load-bearing one and the variants are its consequence.
- **`None` is corrected from "another mechanism" to "no mechanism".** Its doc previously offered "a bearer token
  in a header" as an example of *another way to authenticate* that `None` made representable — but `None` means
  **no** authenticator, so the only honest way to represent a bearer-token authenticator was always a new
  variant, not `None`. The doc now says so and warns against reaching for `None` when the truth is "an
  authenticator I cannot name".
- **Two guards were falsified A-B-A with compiling mutants.** (a) Disabling the encoding refusal
  (`if false && …`) let a body-independent authenticator claim `Base64`; the test failed with the exact message,
  and restoring the condition made it pass. (b) Mutating the manifest guard from `!authenticates()` to
  `!covers_the_body()` made a **truthful** OIDC push declaration refused (`OidcIdToken is an authenticator, so a
  push declaration must be accepted`), which is the over-broad-guard direction the acceptance test exists to
  catch. Both restored byte-identically.
- **The Google connector's prose was corrected in three places** (`google/mod.rs`'s module doc and its call
  site, and `routing.rs`'s trust section) because each said Google's mechanisms "cannot be expressed" — now
  false. `ADR-0074`'s and `ADR-0096`'s rule: **a comment that names another module is a claim about code, and
  code changed.** A test's premise was corrected the same way rather than left green on a stale reason.
- **A limit remains, and it is the honest half of the finding: nothing verifies either mechanism.** This slice
  makes the two schemes *expressible*; it builds no JWKS reader, no certificate-rotation cache, no claim
  checker, and no channel-token comparison. So the authentication gap `ADR-0097` recorded is **narrowed from
  "cannot be declared" to "is declared but unverified"**, and `P5-010` owns closing it. The connector therefore
  declares `Polling`, which is still true of what runs today.
- **A second limit is cardinality.** `WebhookSupport` is one value per connector; Google's push is two
  mechanisms. Nothing here resolves that, and it is named rather than hidden: the revisit condition below is
  the shape that would.

## Alternatives considered

- **Add the provider's mechanisms as one `OidcJwt`/"other" variant.** Rejected: an OIDC ID token and an echoed
  channel token differ in what they authenticate (a signature the provider makes vs. a value *this* connector
  chose), in what a verifier must hold (a JWKS endpoint vs. a stored string), and in strength — folding them
  into one variant would make a scheme unable to say which. The names carry the distinction the same way
  `HmacSha1` is named rather than folded into `HmacSha256`.
- **Reuse `SignatureAlgorithm::None` for both, and move the real mechanism into a comment.** Rejected: `None`
  means *no control is present* and is refused for a push declaration, so this would either keep the endpoints
  undeclarable or force the refusal to be weakened — the very thing `ADR-0054`'s guard exists to prevent.
- **Widen the contract *and* flip Google to `Push`, one mechanism at a time.** Rejected: `WebhookSupport` is a
  single field, so either choice is a partial truth (decision 5), and flipping to `Push` also drags in the two
  webhook readiness items, which the connector is not ready to satisfy because no verifier exists. Correcting
  the prose is the honest move; the declaration waits for the verifier.
- **Make `encoding` optional instead of refusing a mismatch.** Rejected: an `Option<SignatureEncoding>` would
  make "no encoding" and "an encoding the author did not choose" the same value, which is the conflation
  `ADR-0057` records. `Raw` already means exactly "the header value is consumed as it arrived", so it is the
  honest value for a token and no new option is needed.
- **Fold `encoding`'s refusal into `SignatureError::Header`.** Rejected: the remedies differ — a header name is
  a typo in one field, while an encoding on a body-independent authenticator is a wrong model of the mechanism —
  and every other error in this crate is separated on exactly that basis.
- **Give the two families a shared `family()` accessor instead of `covers_the_body`/`is_body_independent`.**
  Rejected: a caller's actual question is a property of the delivery ("are the bytes covered?"), not a label,
  and a label would leave the mapping from name to property to be re-derived at each call site.

## Conditions that would justify revisiting

- **A push verifier is built** (a JWKS reader for the JWT, a stored-value comparison for the channel token),
  which turns decision 5's "unverified" into "verified" and would let the connector declare `Push` — provided
  the cardinality question is settled first.
- **`WebhookSupport` gains a way to declare more than one mechanism** (a set, a map, or one value per watched
  resource), at which point Google's two mechanisms can both be declared and decision 5's cardinality argument
  no longer applies.
- **A third provider arrives with a body-independent authenticator** (a signed URL, an HMAC over a *parsed*
  subset, a mutual-TLS client certificate), which would test whether `covers_the_body` is the right axis to
  organise the enum by or merely the first useful one.
- **The ID-token verification is added**, at which point the `nonce` `P5-002` already carries has a consumer and
  the recorded "prepared seam, not a working feature" in `SCOPE_OPENID`'s doc becomes a feature.
