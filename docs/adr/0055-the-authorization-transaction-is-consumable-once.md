# ADR-0055: The authorization transaction is consumable once, and the redirect cannot be anywhere but loopback

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P5-002` requires "shared OAuth 2.0 Authorization Code plus PKCE flow, state/nonce validation, loopback
callback, refresh rotation, revocation, and secret references". `P5-001` defined the *vocabulary* for all of
those and deliberately held no randomness, no listener, and no token exchange, recording the gap explicitly:
"**`PkceVerifier` is verified, not generated.** There is no RNG dependency, so no code path produces a verifier
[…] Generating one needs a CSPRNG and belongs with the flow that uses it (`P5-002`)."

This slice is that flow. Because it is an external protocol, `AGENTS.md` required the research record first, so
`docs/research/integrations/oauth2-pkce-native-apps.md` was written from the live RFC texts (RFC 7636, RFC 8252,
RFC 9700, with RFC 6749 and RFC 9207 as referenced) before any code. Every rule below cites the sentence it
comes from, because the counter-intuitive ones are exactly the ones a later reader will "simplify".

Four findings shaped the design and were not predictable from the slice description.

1. **The protocol's own rules make several "reasonable" client behaviours wrong.** RFC 9700 §4.2.4 requires a
   code to be invalidated after its first use *and* says a double redemption SHOULD revoke the tokens the first
   attempt issued. So a retry of a request whose answer was lost can **destroy a working grant** — meaning
   "retry the token request" is not a safe default and cannot be one.
2. **`localhost` is worse than it looks.** RFC 8252 §8.3 says it is NOT RECOMMENDED and gives the reason: it
   "avoids inadvertently listening on network interfaces other than the loopback interface. It is also less
   susceptible to client-side firewalls and misconfigured host name resolution." A hostname whose meaning
   depends on a resolver cannot ground the claim "this listener is only reachable from this machine".
3. **PKCE and `state` do overlapping but different jobs, and dropping either is a real regression.** RFC 9700
   §2.1 permits relying on PKCE for CSRF *conditionally* — "Clients that have ensured that the authorization
   server supports PKCE MAY rely on the CSRF protection provided by PKCE" — and §4.7.1 makes confirming that
   support a **MUST**.
4. **Two of my test premises were wrong, and one of them exposed a doc comment that lied.** A comment claimed a
   pathless redirect URI would be refused; the code normalises it to `/`, which is what RFC 3986 §6.2.3 says it
   is. And a scope-loss fixture asserted a loss from a grant that had lost nothing.

## Decision

### 1. A transaction is consumable once, by construction

`AuthorizationTransaction::consume(self, callback)` takes `self` **by value**. RFC 9700 §4.2.4 asks the client
to invalidate the `state` "after its first use at the redirection endpoint", and a by-value receiver makes that
a property of the type rather than a discipline a caller has to remember. There is no expression for "answer the
same transaction twice", which is the strongest available form of the rule.

The transaction carries the verifier, the method, the `state`, the optional `nonce`, **the redirect URI**, the
optional issuer, and the issue instant — one value rather than five arguments, because RFC 8252 §8.10 requires
exactly that grouping: "The native app MUST store the redirect URI used in the authorization request with the
authorization session data (i.e., along with 'state' and other related data)". Loose arguments are the
`P3-005`/`P3-006a` defect — two values that must agree with nothing holding both — and here the disagreement
would be a redirect the client was not listening on.

### 2. A loopback redirect is a type with two hosts, so nothing else is representable

`LoopbackHost` is `V4` (`127.0.0.1`) or `V6` (`[::1]`) and nothing else, so `http://evil.example/cb` and
`http://localhost/cb` cannot be constructed rather than being rejected by a check that a new code path could
bypass. That is a stronger guarantee than validation, and it is why `LoopbackRedirect::parse` refuses a host
that is not one of the two literals rather than pattern-matching.

The **path** is validated rather than typed, because it is genuinely free-form: absolute, ≤256 characters, and
no `?`, `#`, or `..`. The query and fragment refusals are not tidiness — everything after `?` belongs to the
authorization server, so a client that had already put its own parameters there could not tell its own from the
response's, which is the open-redirector shape in RFC 9700 §4.1.2 and §4.11.1.

A pathless URI normalises to `/`, per RFC 3986 §6.2.3. **An earlier revision of this ADR's own doc comment
claimed the opposite**, and the test written from that claim failed — see the fourth Context finding.

### 3. The port varies and nothing else does, in exactly the direction the RFCs specify

`matches_except_port` compares scheme, host, and path and **excludes the port**. It is the comparison RFC 9700
§2.1 mandates for a loopback redirect ("exact string matching except for port numbers in localhost redirection
URIs of native apps") and the one RFC 8252 §7.3 requires to work at all ("The authorization server MUST allow
any port to be specified at the time of the request").

`matches_exactly` also exists, because `registered` (portless, what an app registers) and `listening` (ported,
what it is bound on) are two different documents and a caller needs to be able to say so. The test asserts
**both**: a different port on the same loopback path is accepted, and a different **path** is refused. A
one-sided test would pass on a check that refused everything.

### 4. `state` is always sent, even though PKCE can substitute for it

RFC 9700 §2.1 permits PKCE-as-CSRF-protection only after confirming the server supports PKCE, and §4.7.1 makes
that confirmation a **MUST**. Rather than make the safety of a flow depend on a capability-discovery step that
may be unavailable, `state` is unconditionally present. The cost is one random value; the benefit is that no
configuration leaves the client unprotected, and `parameters()` has no branch in which the state is absent.

The absence is refused in two distinguishable ways: `StateMissing` (a response with no `state`) and
`StateMismatch` (a `state` that is not this transaction's). The **values** are compared in constant time by
`SecretValue::matches`, because the comparison is against attacker-supplied text.

### 5. `nonce` is carried and handed back, and nothing claims to have validated it

RFC 9700 §4.5.3.2 makes the `nonce` meaningful only when an ID Token is issued, and verifying one needs OIDC's
ID-token verification (signature, `iss`, `aud`, `at_hash`), which this slice does not implement. So the
transaction holds the nonce, sends it, and hands it to the `Grant` for a caller that *can* verify — and the
research record's Unresolved Question 5 states plainly that code-injection defence rests on PKCE alone. The
alternative was worse: a method called `validate_nonce` that could not validate anything would be the
"declared but unconstructed" pattern this repository has removed four times.

### 6. The mix-up defence has three states, and only one of them is a refusal

RFC 9700 §2.1 makes a mix-up defence **REQUIRED** once a client can talk to more than one authorization server,
and §4.4.2.1 gives RFC 9207's `iss` as the preferred one. `MixUpDefence` distinguishes:

- `NotNeeded` — the transaction never named an issuer, so no defence was attempted (§4.4.2: "When an OAuth
  client can only interact with one authorization server, a mix-up defense is not required");
- `IssuerConfirmed` — an issuer was stored and the response matched;
- `NotSatisfied` — an issuer was stored and the response carried none. **Not an attack**: RFC 9207's parameter
  is optional, so its absence is not evidence, and a caller that talks to several issuers must refuse it
  itself. `IssuerMismatch` is the only case that aborts, per §4.4.2.1's "MUST abort".

Collapsing `NotSatisfied` into a refusal would break every conforming server that omits `iss`; collapsing it
into `IssuerConfirmed` would report a defence that never ran.

### 7. The listener is a trait whose capabilities are *reported*, and the security ones are separated from the compatibility one

`LoopbackListener` reports its `LoopbackRedirect` and a `ListenerCapabilities`. This crate has no runtime and
opens no sockets — a boundary `P5-001` fixed — so the hardening requirements become values the caller
*achieves*: `loopback_only`, `exclusive_bind`, `closes_after_response`, `both_ip_stacks`.

The first three are RFC 8252 §8.3 and its platform notes B.3 (`SO_EXCLUSIVEADDRUSE`) and B.5 ("SHOULD NOT set
the `SO_REUSEPORT` or `SO_REUSEADDR` socket options"), which are the same rule for two operating systems: **no
second binder on the port**. The fourth is §7.3's both-stacks recommendation, which is a **compatibility**
concern, so `UnmetListenerRequirement::is_security` returns `false` for it. Conflating a single-stack listener
with an off-machine-reachable one would make a real hardening gap dismissible as cosmetic.

The four booleans are `#[allow(clippy::struct_excessive_bools)]` with a reason, because the alternative —
a set of unmet requirements — would have `Default` = empty = "everything is met", so a caller that forgot to
report a capability would silently claim it. Four `bool`s default to `false`, which fails closed. That is a case
where the lint is right in general and wrong here, and the reason is recorded rather than the shape changed.

### 8. `TokenSet` has no access-token field, and that absence is the design

`tools-and-connectors.md` says "No token material in model context, URLs/logs, diagnostics, or normal database
columns". `P5-001` honoured that by giving the manifest and the auth module nowhere to put a token; this slice
extends the same absence to the exchange. `TokenSet` records the lifetime, the granted scopes, the scope change,
and a `SecretRef` that **locates** the refresh material. It does not record `access_token`, and it records
`has_refresh_token: bool` rather than the refresh token's text.

So there is no expression in this crate for handing a token to the next layer, which means a model context, a
log line, a diagnostic, and a column cannot receive one even by mistake. The access token still exists — an
adapter needs it — but in the adapter's local scope for the length of one call, which is what `security.md`
requires. The test asserts the `Debug` rendering contains no `access_token` and no `refresh_token`, which is
what would catch a field being added later.

### 9. A token request's retry-safety is a variant, because the two cases are indistinguishable and opposite

`TokenRequestOutcome` separates `NeverSent` (safe to retry — nothing reached the provider) from
`SentAnswerUnknown` (**not** safe to retry blindly). RFC 9700 §4.2.4 makes the distinction consequential: a
retry of a request whose answer was lost may get `invalid_grant` *and* revoke a grant the first attempt had
already issued. From the request side the two are identical, so a caller must choose, and a variant forces the
choice rather than letting it be inherited from a retry loop's default. Same shape as `P3-001`'s
`RefusedBeforeReaching`/`AmbiguousAfterReaching`, one protocol over.

### 10. Refresh rotation is *detected*, because noticing is what makes replay visible

RFC 9700 §4.14.2 defines rotation as "the authorization server issues a new refresh token with every access
token refresh response", and explains the payoff: if a refresh token is reused by both an attacker and the
legitimate client, "one of them will present an invalidated refresh token, which will inform the authorization
server of the breach". That detection needs the *client* to notice which of the two shapes came back — a client
that kept using its old token would throw the defence away. So `RefreshExchange::classify` reports `Rotated` or
`Refreshed` from `has_refresh_token`, and reports `Rotated` even when the caller supplied no reference for the
new material, because a caller's storage defect must not hide the half of the exchange that makes replay
detectable.

### 11. The transient check outranks the error code, and the ordering is load-bearing

A provider that answers a refresh with HTTP 503 normally answers through the protocol's own error channel, and
a reverse proxy or JSON API framework in front of it will not always pick a code that describes the outage
rather than the request — a 503 carrying `invalid_grant` is a real shape. If the grant check ran first, that
response would send the user to a consent screen **during a provider outage**, which finds the same failure and
looks like a broken connector. So "the provider is unwell" outranks "the server said this code", because the
transport's observation is the more specific one.

**This ordering is the finding of the falsification run.** The first mutation of it SURVIVED, and diagnosing
why showed the *fixture* was weak: it used a `server_error` code, for which both orderings give `Transient`. A
fixture was added — a transient 503 carrying `invalid_grant` — which is the only shape where the two orders
differ, and the guard then falsified properly. A green falsification is worth exactly as much as the mutant's
ability to make the difference observable.

### 12. `invalid_grant` needs the user; a client-side misconfiguration does not

RFC 6749 §5.2 defines `invalid_grant` as the grant being "invalid, expired, revoked, does not match the
redirection URI […] or was issued to another client" — every one of those needs a new authorization, so it is
the one code that maps to a user-visible reauth. `invalid_client` and `unauthorized_client` are deliberately
**not** included: they mean the *client* is misconfigured, so sending someone through a consent screen lands on
the same failure. They map to `Transient`, whose `needs_user` is false and `is_safe_to_retry` is false — the
honest pair for "something is wrong that the user cannot fix".

### 13. A revocation's failure is not the same claim as its success

RFC 7009 §2.2 makes "already invalid" a **success** — the server answers 200 "if the token has been revoked
successfully or if the client submitted an invalid token" — so `RevocationOutcome::AlreadyInvalid` counts as
`is_withdrawn`. `Unsupported`, `Refused`, and `Unreachable` do not: in each the token may still work, and a
caller that assumed otherwise would leave a live credential behind while telling the user it was withdrawn.

That asymmetry is the reason local material is discarded on **every** outcome including the failures: keeping a
credential the user asked to disconnect serves nothing, and a later successful retry needs no local token. An
always-`true` `discards_local_material()` method was written here first and **removed**, because a predicate
with one answer is not a predicate; the obligation moved into the type's documentation where it belongs.

`RevocationKind::Grant` has no `token_type_hint`, because RFC 7009 revokes one token at a time and defines no
hint for "everything" — so a whole-grant revocation is two calls, and a caller has to know that rather than
send a hint no server understands.

### 14. The generated verifier is the RFC's own recipe, and it goes through the validator

`PkceVerifier::generate()` takes 32 random octets and encodes them with **the same unpadded base64url the
challenge uses**, per RFC 7636 §7.1 ("a suitable random number generator create a 32-octet sequence. The octet
sequence [is] then base64url-encoded to produce a 43-octet URL safe string"). 43 characters is
`MIN_PKCE_VERIFIER_CHARS`, so a generated verifier is at the RFC's floor by construction — and it is passed
through `new()` anyway, because the generator and the validator are two implementations of one rule and only
running one through the other catches a divergence.

`plain` remains representable, because RFC 7636 §5 says a server MAY ignore the extension entirely and a caller
may legitimately know it is talking to an old one. What is not representable is selecting it **by accident**:
`code_challenge_method` is always sent as `S256` by a `PkceMethod::S256` transaction, which removes the
default's effect — a server that read an omission as `plain` would compare the verifier directly, which is
exactly the weakness §7.2 exists to prevent.

### 15. Every guard was falsified, one mutation at a time, in an A-B-A design

All 25 guards: **A** (intact) passes, **B** (neutered) fails, **A′** (restored) passes. The two that did not
prove on the first run were both informative — one mutant was invalid (it left its `match` unbalanced), and one
SURVIVED and led to the discovery recorded under Decision 11.

A′ is load-bearing and not ceremony: restoring a file with `Copy-Item` sets its mtime **backwards**, older than
the mutant build cargo just made, so cargo can keep running the stale mutant. That produced a false "SURVIVED"
during `P5-001`; the harness therefore restores with `WriteAllText` and bumps `LastWriteTime` forward.

## Consequences

- `jarvis-connectors` gains two modules (`authorization`, `token`) and 36 tests, for 116 in the crate. It
  remains an adapter depending only on `jarvis-core` + `jarvis-tools`; `getrandom` was added as a dependency
  and was **already in the workspace and the lock file**, so the change adds no package to the tree and cannot
  change `cargo deny`'s verdict.
- `jarvis_core::SecretRef` is consumed rather than restated, so a refresh token's *location* uses the same
  redaction-safe type as every other secret reference in the platform. It is deliberately not `Serialize`,
  which is what keeps it out of a wire contract or a diagnostics payload.
- The `AuthFlow` surface `P5-001` defined is now driven end to end: `AuthFlow::new` → `LoopbackRedirect` →
  `AuthorizationTransaction::begin` → `parameters()` → `consume()` → `Grant`. Every one of the nine OAuth
  requirements in `tools-and-connectors.md`'s list now has a type or a check behind it.
- `DEFAULT_TRANSACTION_SECONDS` (600) and `MAX_ACCESS_TOKEN_SECONDS` (86 400) are bounds with stated reasons;
  the first is the conventional authorization-code lifetime and must outlast a human typing a password plus a
  second factor, and the second admits every conventional `expires_in` (Google 3600, Microsoft 3600–4800)
  while refusing a value that is a server defect or not a token.

## Limits

- **Nothing consumes this crate.** No `Connector` trait, no HTTP client, no socket, no `SecretStore`
  integration, no persistence, and no daemon wiring. This slice defines and drives the *flow*; `P5-004` onward
  is where a vendor and a listener exist.
- **No live verification, deliberately.** This record's contract is a protocol and the only live test would be
  against a real vendor with registered credentials and a browser — i.e. the per-connector smoke test
  (`P5-005`/`P5-007`/`P5-008`) behind a cost and credential gate. Nothing here claims a live verification.
- **`nonce` is carried, not validated.** OIDC ID-token verification is not implemented, so code-injection
  defence rests on PKCE alone (research record Unresolved Question 5).
- **The mix-up defence depends on a vendor sending `iss`.** RFC 9207's parameter is optional, so with a server
  that omits it a client talking to several issuers has only the distinct-redirect-URI fallback — the option
  RFC 9700 §4.4.2.2 says should be used only if nothing else is available (Unresolved Question 1).
- **`Expired` and `Revoked` are the caller's classification.** RFC 6749 §5.2 lists no code that distinguishes
  them, so `vendor_says_revoked` is a parameter: the vendor record supplies it (Unresolved Question 2).
- **Sender-constraining is not implemented.** RFC 9700 §2.2.2 requires refresh tokens for public clients to be
  sender-constrained *or* rotated. Rotation is detected; DPoP (RFC 9449) and mTLS (RFC 8705) are not built, so
  a provider that does **not** rotate is a configuration this client cannot make compliant on its own
  (Unresolved Question 3). This is the strongest reason sender-constraining is on the roadmap.
- **No automatic token-request retry, by design** (Decision 9). A caller that needs one has to implement the
  choice, and the type only tells it the choice exists.
- **No `localhost` support, and no configuration to enable it.** RFC 8252 §8.3 says NOT RECOMMENDED and gives a
  reason that is about the resolver; a caller that needs it must change the type, which is the intended cost.
- **`AuthError::RandomUnavailable` is untestable here.** The platform random source does not fail on demand, so
  the branch that propagates its failure is verified by reading rather than by a test. It is a two-line branch
  and its shape matches `jarvis_core::DecisionNonce::generate`'s, which has the same gap.
- **The token request and the revocation call are not performed.** This module classifies a response the caller
  supplies; the HTTP calls, their timeouts, and their error mapping are the connector's (`P5-005` onward), which
  is what keeps every branch here verifiable as a function of its arguments.
