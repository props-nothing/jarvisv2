# ADR-0065: A protocol type stays correct when a provider disagrees with it

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (Google revocation during connection teardown).
- **Relates to:** `ADR-0055` (the authorization transaction is consumable once), `ADR-0064` (a token answer is
  read from its body), `ADR-0042` (a decision nonce is delivered by file, and taking it consumes it).

## Context

`P5-002` built `RevocationKind` over RFC 7009, and its accessor is a clean reading of that specification:

```rust
pub const fn requires_reauth_afterwards(self) -> bool {
    matches!(self, Self::RefreshToken | Self::Grant)
}
```

`AccessToken` is excluded because RFC 7009's `token_type_hint` **selects which token is revoked** — revoke the
access token and the grant is untouched, so nobody needs to authorize again. That is correct.

Google does not work that way, and says so in two places on one page
(`https://developers.google.com/identity/protocols/oauth2/web-server`, last updated **2026-09-14**):

> "If the token is an access token and it has a corresponding refresh token, the refresh token will also be
> revoked"

> "Revocation removes all OAuth 2.0 scopes previously granted to a project, invalidating any issued access or
> refresh tokens for all clients registered under that project."

So revoking an **access** token on Google destroys the refresh token beside it and removes the project's scopes.
No revocation kind leaves the account usable. A caller that took `RevocationKind::AccessToken
.requires_reauth_afterwards()` at its word would report a disconnect that left the account authorised, and the
user's next call would fail in a way that looks like a broken connector.

The same page adds: **"Following a successful revocation response, it might take some time before the revocation
has full effect."** So a `200` means the request was **accepted**, not that it is **in force**.

This is a general situation, not a one-off: a shared type encodes a *specification*, and a provider may be
narrower or wider than the specification. The question is where the provider's answer lives.

## Decision

**1. The shared type is not changed. It is right about the protocol.**

Editing `RevocationKind` to say Google's answer would make a protocol type provider-specific, and would then be
wrong for the next provider that follows RFC 7009. `jarvis-connectors`' shared modules encode protocol; provider
modules encode providers. The boundary is the same one `ADR-0058` draws for "a provider decision belongs in the
crate that does not own a socket".

**2. The provider's answer is a separate function, and the divergence is asserted.**

`google::revocation::effect_of(kind, has_refresh_token)` returns Google's answer. A test named
`google_disagrees_with_the_protocol_about_revoking_an_access_token` asserts **both sides** — that the protocol
type says `false` and that Google says `true` — and then asserts they differ. So the contradiction is a
*recorded* fact rather than a comment: changing either side fails a test, and the reader must decide which
document wins.

**3. `requires_reauth` is derived from the grant's removal, not stored beside it.**

```
pub const fn requires_reauth(self) -> bool {
    self.scopes_removed
}
```

With the scopes gone there is no consent to reuse and no refresh token to mint from, so needing a person follows
from the grant. A separate stored field would be a second value that must agree with the first — the defect class
this repository keeps recording.

**4. "Invalidated" and "there was none" are different states, not a boolean.**

The lint caught this: the first version of `RevokedEffect` had five booleans, and
`refresh_invalidated: false` could not distinguish *the refresh token survived* from *the account never had one*.
Those call for opposite next steps, and one of them would tell a user their refresh token was revoked when it
never existed. So `MaterialState` has three variants — `Invalidated`, `Absent`, `Untouched` — where `Untouched`
is never produced for this provider and exists so "we do not know" is representable rather than rounded.

**5. "It might take some time" is a state, not a warning.**

`EffectTiming { Immediate, MayTakeTime }` rather than `takes_effect_later: bool`, because a `200` being
*accepted* rather than *in force* is the whole content of the distinction and a caller must act on it.

**6. An unreachable provider is never reported as revoked.**

`parse_revocation_answer(status, body, reached)` takes `reached` separately from the status, because a transport
cannot say "the provider refused" and "nothing answered" with one value. `Unreachable.is_withdrawn()` is `false`,
so a caller cannot tell a user their access was withdrawn when the request never arrived — the overclaim
direction worth designing against. A `200` that was never received must not read as success.

**7. A `200` does not report "already invalid", because it cannot.**

RFC 7009 §2.2 makes `200` cover both success and "the client submitted an invalid token", and Google's endpoint
follows that `200`/`400` split. `RevocationOutcome::AlreadyInvalid` is therefore **not derivable** from a
response, and inventing it from the status would be reading a fact out of a response that does not carry it. The
variant stays reachable — a provider may report it out of band, or a later `invalid_grant` establishes it — and
the gap is stated rather than left as an omission.

**8. A gateway's status classifies as a refusal, and a helper names the documented pair.**

The endpoint produces `200` and `400`. A `502` from a proxy is a refusal — the provider *was* reached through
something — not an outage-success and not `Unreachable`. `status_is_documented` lets a caller tell the
documented pair from a layer in front of the endpoint.

**9. The token hint is sent and its meaning is narrower than a caller might assume.**

RFC 7009's hint "MAY" be ignored by the server, so it is advisory and never used to select anything locally.
Sending it is right: it is the one part of the request that tells a conforming server which token is meant. The
trap is on the **reading** side — assuming the hint means "only revoke this one" imports RFC 7009 scope semantics
into a provider that documents project-wide removal — and `effect_of` is where that is corrected.
`RevocationKind::Grant` sends no hint because RFC 7009 defines none.

**10. Four guards were falsified with compiling mutants.**

Not deriving `requires_reauth` (3 tests detected), collapsing `Absent` into `Invalidated` (2), claiming an
immediate effect (2), and letting an unreachable provider read as `Revoked` (2).

## Consequences

- **A protocol type and a provider's deviation can both be right.** The specification stays reusable and the
  provider's answer is where a caller will look for it, with the divergence under test.
- **A disconnect can now be reported honestly.** "The provider accepted it; it may take time; you will need to
  authorise again" is a sentence the types can support, and none of its three clauses is invented.
- **The lint produced a design improvement.** `struct_excessive_bools` was a real signal here rather than
  noise: the booleans were collapsing states that differ.
- **The blast radius is recorded where a caller reads it**, not only in the research record: revocation can
  withdraw grants belonging to other accounts and other clients in the same Cloud project.

## Limits

- **No request is sent and nothing has been revoked.** There is no transport for a form `POST`, so no token has
  been revoked and no `200` has been seen. The behaviour is derived from the documentation and the RFC.
- **The divergence is documented, not measured.** The claim that Google revokes the paired refresh token comes
  from its documentation; confirming it would need a live grant and deliberately destroying it. So a provider
  whose *behaviour* differs from its *documentation* would not be caught here.
- **`has_refresh_token` is the caller's claim about its own store.** This module takes it as a parameter, so a
  caller that does not know whether it holds a refresh token must answer anyway — and answering `false`
  incorrectly narrows the reported effect.
- **`RevocationKind::Grant` cannot actually be performed in one call to a strictly-conforming server.** RFC 7009
  revokes one token at a time and defines no hint for "everything", so two calls are needed there. On Google one
  call achieves it, which is why `effect_of` reports the widest blast radius for every kind — but that is a
  provider property being relied on, and it is recorded rather than encoded in the shared type.
- **Nothing handles the multi-account consequence.** The module reports that the blast radius is the project; a
  caller that revokes for one account must still decide what to tell the *other* accounts whose grants were
  withdrawn. No type here holds a set of affected accounts.
- **`takes_effect_later` has no duration.** Google says "some time" and names no bound, so a caller cannot
  schedule a verification. Nothing re-checks a revocation after the fact.
- **Revocation is not wired to anything.** No route, no command, and no account teardown path calls this module,
  so a disconnect is still not something a deployment can perform.
