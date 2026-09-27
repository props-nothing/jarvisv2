---
integration: oauth2-pkce-native-apps
status: implemented
last_verified: 2026-09-27
owners: []
selected_spec_version: "RFC 7636 (PKCE); RFC 8252 BCP 212; RFC 9700 BCP 240; RFC 6749; RFC 9207"
selected_sdk: none (no SDK; the protocol is small and an SDK would hide the checks that matter)
---

# OAuth 2.0 Authorization Code + PKCE for native apps

## Scope

**In scope:** the OAuth 2.0 Authorization Code grant performed by a **desktop (native) public client** using
**PKCE**, the `state` and `nonce` values bound to a short-lived setup transaction, **exact** redirect URI
validation with the loopback-host exception, loopback listener requirements, **refresh-token rotation and
replay detection**, revocation, and the rule that no token material reaches a model, a URL, a log, a
diagnostic, or a normal database column.

**Out of scope and deliberately not researched to implementation depth:** confidential-client
authentication (`client_secret`/Private Key JWT/mTLS), DPoP and mTLS sender-constraining, OpenID Connect
(discovery, ID tokens, JARM), pushed authorization requests (PAR), dynamic client registration, rich
authorization requests, form-post response mode, and device authorization. Each is named under Decisions or
Unresolved Questions where it affects a chosen design.

This is a **protocol** record, not a vendor record. The per-vendor records (`google.md`,
`microsoft-graph.md`, `github.md`) are required separately before each connector, because they supply the
endpoints, scopes, and provider-specific error shapes this record deliberately does not.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| PKCE specification | https://www.rfc-editor.org/rfc/rfc7636.txt (RFC 7636, Standards Track, Sept 2015) | 2026-09-27 | normative: verifier alphabet and bounds, S256/plain transformations, request parameters, the §4.4.1 error rule, Appendix B's S256 vector, Appendix A's base64url note |
| OAuth 2.0 for Native Apps | https://www.rfc-editor.org/rfc/rfc8252.txt (RFC 8252, BCP 212, Oct 2017) | 2026-09-27 | normative: browser-not-webview, loopback redirect shape and ephemeral port, `state` CSRF rule, mix-up defence, client authentication posture, platform notes (B.3 Windows `SO_EXCLUSIVEADDRUSE`, B.5 Linux "SHOULD NOT set `SO_REUSEPORT`/`SO_REUSEADDR`") |
| Best Current Practice for OAuth 2.0 Security | https://www.rfc-editor.org/rfc/rfc9700.txt (RFC 9700, BCP 240, Jan 2025) | 2026-09-27 | normative: exact redirect matching, PKCE mandatory for public clients, PKCE downgrade mitigation, authorization code one-time use + revoke-on-double-redemption, refresh rotation for public clients, mix-up defence via `iss`, `state` single use, ROPC forbidden, implicit grant deprecated |
| OAuth 2.0 Authorization Framework | RFC 6749 (referenced normatively throughout; not re-fetched in full) | 2026-09-27 | referenced for the authorization/token endpoint parameter names and the §4.1.3 redirect_uri equality rule, as quoted by RFC 9700 §4.5.2 |
| OAuth 2.0 Authorization Server Issuer Identification | RFC 9207 (referenced by RFC 9700 §4.4.2.1) | 2026-09-27 | referenced for the `iss` response parameter as the mix-up countermeasure |
| `llms.txt` for these specs | `not found` — the IETF publishes per-RFC text and HTML, not an `llms.txt` index. The record therefore cites the RFC text files directly, which are the normative artifacts. | 2026-09-27 | discovery |

**No SDK selected.** RFC 7636 is roughly four pages of normative content plus two appendices. An OAuth
client library would supply the token request *and* hide exactly the checks this slice exists to make
visible — the redirect-URI equality rule, the single-use code, the rotation detector. The protocol is small
enough to call directly, and the caller (the connector) is where provider-specific error shapes are handled
anyway. This matches `P3-007`'s decision to keep `jarvis-mcp` free of an SDK so its authority rules were
verifiable as functions of their arguments.

## Verified Contract

### Operations And Transport

Two network operations, both caller-supplied as *endpoints* rather than constructed here:

| Operation | Method | Parameters | Where |
| --- | --- | --- | --- |
| Authorization request | GET (browser navigation) | `response_type=code`, `client_id`, `redirect_uri`, `scope`, `state`, `code_challenge`, `code_challenge_method`, (OIDC) `nonce` | authorization endpoint |
| Token request | POST | `grant_type=authorization_code`, `code`, `redirect_uri` (**REQUIRED if it was in the authorization request**, RFC 6749 §4.1.3), `client_id`, `code_verifier` | token endpoint |
| Refresh | POST | `grant_type=refresh_token`, `refresh_token`, `client_id`, `scope` (optional, may only narrow) | token endpoint |
| Revocation | POST | `token`, `token_type_hint`, `client_id` | revocation endpoint (RFC 7009, vendor-provided) |

The authorization response arrives as a redirect to the loopback listener carrying either `code`+`state` or
`error`+`error_description`(+`state`). The token responses are JSON.

**No framing, no streaming, no pagination, no events.** Ordering is inherent: one request, one response, and
the code is bound to a single transaction.

### Authentication And Authorization

- **Public client.** RFC 8252 §8.4: a native app is a public client and **MUST** be registered as one. §8.5:
  a statically embedded shared secret is not confidential, "servers MUST treat the client as a public client
  […] and not accept the secret as proof of the client's identity". So the presence of a `client_secret` field
  in a token response changes **nothing** about what this client may claim.
- **PKCE is mandatory, not optional.** RFC 8252 §6: "Public native app clients MUST implement [PKCE]". RFC 9700
  §2.1.1: "Public clients MUST use PKCE". §2.1.1 also requires the challenge be "transaction-specific and
  securely bound to the client and the user agent", and encourages servers to detect **constant** challenge
  values.
- **S256 only in new implementations.** RFC 7636 §4.2: "If the client is capable of using S256, it MUST use
  S256". RFC 9700 §2.1.1: "clients SHOULD use PKCE code challenge methods that do not expose the PKCE verifier
  in the authorization request. […] Currently, S256 is the only such method."
- **No downgrade, ever.** RFC 7636 §7.2: "Clients MUST NOT downgrade to plain after trying the S256 method.
  […] an error when 'S256' is presented can only mean that the server is faulty or that a MITM attacker is
  trying a downgrade attack."
- **Verifier shape.** RFC 7636 §4.1: 43–128 characters from `[A-Z]/[a-z]/[0-9]/-/./_/~`. §7.1: "SHOULD create
  a code_verifier with a minimum of 256 bits of entropy […] a suitable random number generator create a
  32-octet sequence. The octet sequence [is] then base64url-encoded to produce a 43-octet URL safe string".
  §4.2's ABNF for `code-challenge` is the *same* alphabet and the same 43–128 bounds.
- **S256 definition.** `code_challenge = BASE64URL-ENCODE(SHA256(ASCII(code_verifier)))`. §4.6 restates it as
  `BASE64URL-ENCODE(SHA256(ASCII(code_verifier))) == code_challenge`, and a mismatch is `invalid_grant` (§4.6).
  Appendix B publishes the exact test vector used in this record's tests.
- **base64url.** §3 defines it as RFC 4648 §5's URL-safe alphabet "with all trailing '=' characters omitted
  […] and without the inclusion of any line breaks, whitespace, or other additional characters". Appendix A
  gives the transformation from standard base64: strip trailing `=`, `+`→`-`, `/`→`_`.
- **`state` is REQUIRED for a client that does not rely on PKCE for CSRF.** RFC 9700 §2.1: a client that has
  confirmed PKCE support "MAY rely on the CSRF protection provided by PKCE"; otherwise one-time CSRF tokens in
  `state` "securely bound to the user agent MUST be used". §4.7.1 warns that with PKCE-as-CSRF-protection the
  client "MUST ensure that the authorization server supports PKCE **before** using PKCE for CSRF protection".
  RFC 8252 §8.9: include "a high-entropy secure random number in the 'state' parameter […] and reject any
  incoming authorization responses without a state value that matches a pending outgoing authorization
  request".
- **`state` is single use.** RFC 9700 §4.2.4: "The state value SHOULD be invalidated by the client after its
  first use at the redirection endpoint."
- **`nonce` is an OpenID Connect value, not an OAuth one.** RFC 9700 §4.5.3.2: it protects against
  authorization code **injection** (not against a direct token-endpoint redemption, which PKCE covers), and it
  only works if the client validates the nonce in the ID Token "even if another ID Token was obtained from the
  authorization response" and disregards all tokens until that check succeeds. It is meaningful only when an
  ID token is issued, so it is an OIDC concern and this record treats it as **optional and unverifiable in
  pure OAuth**.
- **Redirect URI matching is EXACT, with one exception.** RFC 9700 §2.1: "authorization servers MUST utilize
  exact string matching except for port numbers in localhost redirection URIs of native apps". §4.1.3 repeats
  it and refers to RFC 3986 §6.2.1 Simple String Comparison. §4.1's attacks target pattern matching —
  `https://*.somesite.example/*` admitting `https://attacker.example/.somesite.example` and subdomain takeover.
- **Loopback redirect shape.** RFC 8252 §7.3: `http://127.0.0.1:{port}/{path}` or `http://[::1]:{port}/{path}`,
  **`http` scheme, loopback IP literal, an ephemeral port chosen at request time**. The server "MUST allow any
  port"; the client "SHOULD NOT assume that the device supports a particular version of the Internet Protocol"
  and it is "RECOMMENDED that clients attempt to bind to the loopback interface using both IPv4 and IPv6 and use
  whichever is available". §8.3: open the port only for the authorization request and close it once the response
  arrives; listen on the loopback interface only. §8.3 also: `localhost` is **NOT RECOMMENDED** — "Specifying a
  redirect URI with the loopback IP literal rather than localhost avoids inadvertently listening on network
  interfaces other than the loopback interface. It is also less susceptible to client-side firewalls and
  misconfigured host name resolution".
- **Listener hardening.** RFC 8252 B.3 (Windows): "apps SHOULD set the 'SO_EXCLUSIVEADDRUSE' socket option to
  prevent other apps binding to the same socket". B.5 (Linux): "Apps SHOULD NOT set the 'SO_REUSEPORT' or
  'SO_REUSEADDR' socket options in order to prevent other apps binding to the same socket." Both are
  platform-specific and both are about the same property: **no second binder on the port.**
- **The response must be received on the URI that was sent.** RFC 8252 §8.10, and it is REQUIRED: "it is
  REQUIRED that a unique redirect URI is used for each authorization server used by the app […] and that
  authorization responses are rejected if the redirect URI they were received on doesn't match the redirect URI
  in an outgoing authorization request. […] The native app MUST store the redirect URI used in the authorization
  request with the authorization session data (i.e., along with 'state' and other related data) and MUST verify
  that the URI on which the authorization response was received exactly matches it."
- **Mix-up.** RFC 8252 §8.10 and RFC 9700 §2.1/§4.4.2: when a client talks to **more than one** authorization
  server a mix-up defence is **REQUIRED**. Preferred: the `iss` response parameter per RFC 9207. Alternative:
  distinct redirect URIs per issuer, which RFC 9700 §4.4.2.2 says "SHOULD therefore only be used if other
  options are not available" because an attacker can register a new client at the honest server using the
  redirect URI the client assigned to the attacker's server. RFC 9700 §4.4.2 warns that "just storing the
  authorization server URL is not sufficient" — an attacker can present an honest authorization endpoint and a
  token endpoint of their own.
- **The authorization response MUST NOT travel unencrypted** except over loopback. RFC 9700 §2.6:
  "Authorization servers MUST NOT allow redirection URIs that use the http scheme except for native clients
  that use loopback interface redirection as described in Section 7.3 of RFC 8252."
- **Browser, never a webview.** RFC 8252 §4/§8.12: native apps MUST NOT use embedded user-agents; §8.12
  documents that an embedded agent "can record every keystroke entered in the login form".

### Limits And Failure Semantics

- **The authorization code is single use.** RFC 9700 §4.2.4: "authorization codes MUST be invalidated by the
  authorization server after their first use at the token endpoint", and when a code is redeemed twice the
  server "SHOULD revoke all tokens issued previously based on that code".
- **PKCE downgrade, server side.** RFC 9700 §4.8.2: if there was **no** `code_challenge` in the authorization
  request, "a request to the token endpoint containing a code_verifier is rejected".
- **Refresh replay.** RFC 9700 §2.2.2: refresh tokens for public clients "MUST be sender-constrained or use
  refresh token rotation". §4.14.2 describes rotation precisely: "the authorization server issues a new refresh
  token with every access token refresh response. The previous refresh token is invalidated, but information
  about the relationship is retained". The detection property: "If a refresh token is compromised and
  subsequently used by both the attacker and the legitimate client, one of them will present an invalidated
  refresh token, which will inform the authorization server of the breach." Notably **which party** is
  legitimate cannot be determined, and the remedy is to revoke the active token and force a fresh
  authorization. §4.14.2 also: refresh tokens **MUST** be bound to the consented scope and resource servers;
  they SHOULD expire after inactivity.
- **Errors.** Authorization-endpoint errors arrive as `error` (+`error_description`) per RFC 6749 §4.1.2.1;
  token-endpoint errors as a JSON body with `error` per §5.2. PKCE's own error rules: a missing challenge when
  required is `invalid_request` (RFC 7636 §4.4.1); a transformation the server does not support is
  `invalid_request`; a verifier mismatch at the token endpoint is `invalid_grant` (§4.6).
- **No rate limits, quotas, or idempotency are defined by the protocol.** The token endpoint's behaviour under
  retry is vendor-specific, which is why this record does not claim one. **A token request is not idempotent in
  the protocol's terms**: a code is single use, so retrying a request whose response was lost will fail with
  `invalid_grant` — and per §4.2.4 may *revoke* the tokens the first attempt issued. That asymmetry is the
  single most consequential operational fact in this record and is why the slice distinguishes
  **"never sent"** from **"sent, answer unknown"**.
- **No provider request IDs** are defined; vendors add them, so the connector supplies them.

### Data And Compliance

- **What leaves JARVIS** during the flow: the authorization request (client_id, redirect URI, scopes, state,
  challenge) goes to the *browser*, which is an external user agent; the token request carries the code and
  verifier. The authorization **code**, **access token**, **refresh token**, and **verifier** are all
  credentials.
- **Storage rule (JARVIS, from `tools-and-connectors.md`):** "No token material in model context, URLs/logs,
  diagnostics, or normal database columns." The RFCs supply the mechanism that makes this achievable: the
  verifier is generated per request and consumed immediately, the code is single use, and the refresh token is
  the only long-lived credential — so the refresh token is the one value that must reach a secret store.
- **The `state` and `nonce` are sensitive but short-lived.** RFC 9700 §4.2.4 also notes that if `state` leaks in
  a `Referer` header the CSRF protection is lost, and the client-side mitigation is invalidating it on first
  use — which is exactly why the "consume on first use" rule is a *security* rule and not tidiness.
- **Residency/retention/pricing:** not applicable; no vendor is called by this record.

### Versions And Deprecations

- RFC 7636 (2015) is current and unmodified. RFC 8252 (2017) is current. RFC 9700 (Jan 2025) is the newest
  normative BCP and **updates RFC 6749, 6750, 6819**.
- RFC 9700 **deprecates** the implicit grant (`response_type=token`, §2.1.2: "clients SHOULD NOT use") and
  **forbids** the resource owner password credentials grant (§2.4: "MUST NOT be used"). Both are absent from
  the design rather than disabled by a flag.
- OAuth 2.1 (`draft-ietf-oauth-v2-1`) is **in progress** and "will incorporate security recommendations from
  this document" per RFC 9700 §1. Following RFC 9700 today is therefore the migration-safe position.
- No errata affecting PKCE's transformations or Appendix B were observed on 2026-09-27.

## JARVIS Mapping

| Provider concept | JARVIS type | Note |
| --- | --- | --- |
| `code_verifier` | `PkceVerifier` (existing) + **`PkceVerifier::generate()`** (new) | generation was the gap `P5-001` recorded |
| `code_challenge` + method | `PkceChallenge` (existing) | S256 by default; `plain` constructible for a *known-old* server only |
| `state`, `nonce` | `SecretValue` (existing) + **`AuthorizationTransaction`** | equality-compared, never displayed, consumed once |
| the pending request as a whole | **`AuthorizationTransaction`** | binds verifier + state + nonce + redirect URI + issuer + issue instant |
| `redirect_uri` | **`LoopbackRedirect`** | the URI **and** the bound port; validates against RFC 8252 §7.3 |
| loopback listener | **`trait LoopbackListener`** | a port the crate does not own (no sockets here) |
| the authorization response | **`AuthorizationResponse`** / **`AuthorizationRefusal`** | validated against the stored transaction |
| token endpoint reply | **`TokenSet`** | `access_token` is **not** stored as a field; only a lifetime and the refresh material's reference |
| refresh rotation | **`RefreshExchange`**, `RefreshOutcome` (existing) | detects the rotated/replayed/lost cases |
| revocation | **`RevocationKind`**, `RevocationOutcome` | distinguishable "revoked" from "was already invalid" |
| `iss` | **`AuthorizationResponse::issuer_matches`** | RFC 9207, the mix-up countermeasure |
| scope drift | `ScopeSetChange` (existing) | already implemented in `P5-001` |

## Decisions

1. **S256 is the default and `plain` cannot be selected by accident.** `PkceMethod::default()` is `S256`, and
   the S256 challenge is verified against **RFC 7636 Appendix B's published vector** rather than against a
   second implementation of the same arithmetic. `plain` remains constructible because RFC 7636 §5 says a
   server MAY ignore the extension entirely and a caller may legitimately know it is talking to an old one, but
   the constructor that *selects* it records why.
2. **A transaction is single-use and cannot be re-answered.** `AuthorizationTransaction::consume(response)`
   takes `self` by value, so a second answer is not expressible. RFC 9700 §4.2.4's "SHOULD be invalidated by
   the client after its first use" becomes a property of the type rather than a caller's discipline.
3. **The verifier is generated, never accepted from a caller, for a real flow.** `PkceVerifier::new` (from
   `P5-001`) exists to *check* a supplied value and is what the tests use; `generate()` is the production path
   and is the only place `getrandom` is called in this crate.
4. **`state` is always sent, even though PKCE can substitute for CSRF.** RFC 9700 §2.1 permits relying on PKCE
   for CSRF **only after confirming the server supports PKCE**, and §4.7.1 requires that confirmation. Rather
   than make the safety of a flow depend on a capability discovery step that may be unavailable, the `state`
   parameter is unconditionally present. The cost is one random value; the benefit is that no configuration
   makes the client unprotected.
5. **`nonce` is carried but its validation is declared unverifiable in pure OAuth.** RFC 9700 §4.5.3.2 makes
   the nonce meaningful only when an ID Token is issued. The transaction holds it and reports it, and the
   token response exposes whether an ID token was present — but the crate does **not** claim to have validated
   a nonce, because it cannot without OIDC's ID-token verification.
6. **Exact redirect matching, with the loopback port as the only variable.** A response is accepted only when
   the URI it arrived on equals the stored one except for the port, and the port comparison is only permitted
   for a loopback host. This is RFC 9700 §2.1's wording applied literally.
7. **`localhost` is refused as a loopback host.** RFC 8252 §8.3 says it is NOT RECOMMENDED and gives the
   reason (it can resolve to a non-loopback interface, so the listener may accept a connection from off the
   machine). Accepting it would be accepting a hostname whose meaning depends on the resolver, which is the
   opposite of the "loopback means 127.0.0.1/::1" property the whole security argument rests on.
8. **The listener is a trait, and the crate never opens a socket.** `P5-001` established that this crate
   depends only on `jarvis-core` + `jarvis-tools`, and a socket is a platform facility with its own hardening
   requirements (RFC 8252 B.3's `SO_EXCLUSIVEADDRUSE`, B.5's "SHOULD NOT set SO_REUSEPORT"). The trait states
   the two properties that matter — **loopback only** and **exclusive binding** — as values the caller
   reports, so the hardening is a declared capability rather than an assumption, which is the same shape
   `ADR-0041` used for sandbox guarantees.
9. **A refresh failure is classified, and "the refresh token is gone" is separate from "the exchange failed".**
   `RefreshOutcome` already distinguishes `Refreshed`/`Rotated`/`Expired`/`Revoked`/`Transient`; `P5-002` adds
   the exchange that *produces* one of those from a response, and the rotation **detector** (a new refresh
   token present ⇒ `Rotated`, absent ⇒ `Refreshed`) because RFC 9700 §4.14.2's replay detection depends on
   noticing which one arrived.
10. **A token request is never retried automatically, and the type says why.** RFC 9700 §4.2.4 revokes tokens
    issued from a code redeemed twice, so a blind retry of a request whose answer was lost can destroy a
    working grant. `TokenRequestOutcome` distinguishes `NeverSent` from `SentAnswerUnknown` so a caller has to
    make that choice explicitly rather than inherit a retry loop's default.
11. **No `AccessToken` type.** `P5-001`'s central absence is preserved: the access token from a response is
    not stored in a field. What is stored is its **lifetime** (`expires_in`) and the refresh material's
    reference, so "the token" is never a value this crate can hand to a model, a log, or a column.

## Rejected Alternatives

- **An OAuth client library.** Rejected in Official Sources above: it would perform the token request and hide
  the redirect-equality, single-use-code, and rotation checks that this slice exists to make explicit. The
  protocol is four pages.
- **The implicit grant.** Deprecated by RFC 9700 §2.1.2, unprotectable by PKCE (RFC 8252 §8.2), and cannot
  refresh without user interaction.
- **Resource owner password credentials.** Forbidden by RFC 9700 §2.4.
- **`plain` as the default.** RFC 7636 §7.2: `plain` "SHOULD NOT be used and exists only for compatibility",
  and it does not protect the verifier against an eavesdropper (§7.2).
- **Relying on PKCE alone instead of `state`.** Permitted by RFC 9700 §2.1 *conditionally* on having confirmed
  PKCE support, and the client-side rule in §4.7.1 is a **MUST** about that confirmation. Decision 4 keeps
  `state` unconditionally rather than making safety depend on a discovery step.
- **Distinct redirect URIs as the mix-up defence.** RFC 9700 §4.4.2.2 says it "SHOULD therefore only be used if
  other options are not available" and explains the circumvention (register a new client at the honest server
  using the other URI). The `iss` parameter is the adopted defence.
- **A fixed loopback port.** RFC 8252 §7.3 requires the server to accept any port and says the client obtains
  "an available ephemeral port from the operating system at the time of the request". A fixed port is
  guessable and collides with another instance.
- **`localhost` as the redirect host.** RFC 8252 §8.3, NOT RECOMMENDED, and it reintroduces name resolution
  into a security decision. See Decision 7.
- **A webview/embedded browser.** RFC 8252 §8.12 — the host app can record every keystroke.
- **Storing the access token so a later call can reuse it.** Contradicts `tools-and-connectors.md`'s storage
  rule and Decision 11; the adapter holds it for the length of one call.

## Verification Plan

**The cheapest test that would disprove the central assumption:** the S256 challenge for RFC 7636 Appendix B's
published verifier must equal Appendix B's published challenge **exactly**. If our base64url or our digest were
wrong in any way, this fails — and a round-trip test could not catch it, because a self-consistent wrong
encoding would round-trip perfectly. This is the same technique `P4-009` used when it asserted the documented
pgvector text form literally rather than only round-tripping it.

**Contract tests (offline, no network, no sockets):**

- **PKCE derivation against the RFC's vector**, plus the `plain` transformation, plus the alphabet/bound
  refusals at both ends.
- **base64url** against RFC 7636 Appendix A's own worked example (`3 236 255 224 193` → `A-z_4ME`), because
  that is the one place the RFC states the transformation for values with every padding length.
- **Generated verifiers**: 1000 of them are the right length, are in the alphabet, and are **distinct** — a
  generator that returned a constant would pass every shape assertion.
- **Transaction single use**: `consume` takes `self`, and a test proves a second answer is refused (via the
  state comparison) rather than accepted.
- **State mismatch, missing state, and an attacker-supplied state** are three distinct refusals with distinct
  reasons, because RFC 9700 §4.7.1's protection is exactly "reject any incoming authorization responses without
  a state value that matches a pending outgoing authorization request".
- **Redirect equality**: same URI accepted; a different **path** refused; a different **port on a loopback
  host** accepted; a different **port on a non-loopback host** refused; a different **scheme** refused;
  `localhost` refused as a loopback host.
- **`iss` mismatch** refused when the transaction named an issuer, and **absence of `iss`** is a distinct
  outcome from a mismatch (RFC 9207's `iss` is optional, so its absence is not evidence of an attack).
- **Token response**: `expires_in` absent/zero/negative handled; a response that is an authorization-endpoint
  error shape is refused as an authorization error rather than parsed as a token.
- **Rotation detection**: a response with a new refresh token is `Rotated`; one without is `Refreshed`; an
  `invalid_grant` is `Expired` or `Revoked` per the vendor's documented signal; a 5xx/transport failure is
  `Transient`. Also: a **rotated** response for a transaction that did not request rotation is still `Rotated`,
  because noticing is the safety property.
- **Revocation**: revoked, already-invalid, and unsupported-by-provider are three outcomes, since a caller's
  remedy differs.
- **No token material in `Debug`/`Display`**: a sweep over every new type's `Debug` for the token text, the
  verifier text, and the state text.
- **Falsification** of each new guard, one mutation per guard, A-B-A (intact passes → neutered fails →
  restored passes) per `/memories/debugging.md`.

**Live smoke test:** **none, and deliberately.** This record's contract is a protocol, and the only live test
would be against a real vendor — which requires a vendor, registered credentials, and a browser, i.e. it is the
per-connector smoke test (`P5-005`, `P5-007`, `P5-008`) with a cost and credential gate. Nothing here claims a
live verification.

## Unresolved Questions

1. **Does a given vendor's token endpoint return `iss`?** RFC 9207's parameter is optional and a client cannot
   require it. **Impact:** without `iss`, a client talking to several issuers falls back to distinct redirect
   URIs — the option RFC 9700 says should be used only if nothing else is available. **Blocked capability:**
   nothing in this slice; the transaction records whether `iss` was present and refuses a *mismatch*. Whether
   JARVIS must synthesise per-issuer redirect paths is answered per connector, with the vendor's metadata.
2. **How does a vendor signal "the refresh token was revoked" versus "the grant expired"?** RFC 9700 §4.14.2
   defines the server's *behaviour* but not a distinguishing error. **Impact:** `Expired` and `Revoked` may both
   arrive as `invalid_grant`, in which case the classification is the connector's, informed by the vendor's
   record. **Blocked capability:** nothing — both are `needs_user`, and `RefreshOutcome::needs_user` is already
   true for both.
3. **Does the vendor implement rotation?** If not, RFC 9700 §2.2.2's requirement falls on sender-constraining,
   which this slice does not implement (DPoP/mTLS are out of scope). **Impact:** an unrotated refresh token for
   a public client is exactly the configuration §2.2.2 forbids. **Blocked capability:** a *compliant* production
   connector against a non-rotating provider — recorded here rather than assumed away, and it is the strongest
   reason DPoP is on the roadmap.
4. **What is the vendor's retry behaviour for a token request whose response was lost?** RFC 9700 §4.2.4 says a
   double redemption SHOULD revoke previously-issued tokens, but not every server implements it. **Impact:** the
   safety of any retry is vendor-specific. **Blocked capability:** automatic token-request retry — deliberately
   not implemented (Decision 10).
5. **OIDC ID-token verification** (signature, `iss`, `aud`, `nonce`, `at_hash`) is **not implemented**, so a
   nonce is carried but not validated. **Impact:** code-*injection* defence rests on PKCE alone, which RFC 9700
   §4.5.3.1 says is the "most obvious solution" and sufficient for OAuth. **Blocked capability:** OpenID Connect
   sign-in as such, and `nonce`-based injection defence.

## Verification Log

| Date | Check | Result |
| --- | --- | --- |
| 2026-09-27 | RFC 7636 text fetched live from rfc-editor.org; §4.1/4.2/4.3/4.5/4.6/7.1/7.2, Appendix A and Appendix B read | Verifier bounds 43–128 and the unreserved alphabet confirmed; S256 = BASE64URL-ENCODE(SHA256(ASCII(v))) confirmed; Appendix A example `3 236 255 224 193` → `A-z_4ME` captured; Appendix B vector (`dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk` → `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM`) captured for the tests |
| 2026-09-27 | RFC 8252 text fetched live; §6, §7.1/7.2/7.3, §8.1/8.3/8.4/8.5/8.9/8.10/8.12, Appendix A/B.3/B.5 read | Loopback shape (`http://127.0.0.1:{port}/{path}`), "server MUST allow any port", both-IP-stack recommendation, `localhost` NOT RECOMMENDED with its reason, `SO_EXCLUSIVEADDRUSE` (Windows) / SHOULD NOT set `SO_REUSEPORT`/`SO_REUSEADDR` (Linux), the §8.10 REQUIRED rule to store the redirect URI with the session, §8.5's "MUST treat the client as a public client", §8.12's ban on embedded user-agents |
| 2026-09-27 | RFC 9700 text fetched live; §2.1/2.1.1/2.1.2/2.2.2/2.4/2.6, §4.1/4.1.3, §4.2.4, §4.4.2/4.4.2.1/4.4.2.2, §4.5.3.1/4.5.3.2, §4.7.1, §4.8.2, §4.14.2 read | Exact redirect matching with the loopback-port exception; PKCE mandatory for public clients; downgrade mitigation; code single-use + revoke-on-double-redemption; refresh rotation for public clients and its replay-detection mechanism; `state` single use; `iss` as the preferred mix-up defence with the explicit warning that distinct redirect URIs are second-best; ROPC forbidden; implicit grant deprecated; RFC 9700 updates 6749/6750/6819 and OAuth 2.1 is still a draft |
| 2026-09-27 | Dependency inventory: `getrandom` 0.4.3 and `sha2` 0.10.9 already in the workspace and in `Cargo.lock`; `getrandom::fill` already used by `jarvis-core::approval` and `jarvis-core::credential` | **No new dependency is needed.** The generation this slice adds reuses the pattern and the packages already present, so the change adds no package to the tree and cannot change `cargo deny`'s verdict |
| 2026-09-27 | Falsification: all new guards mutated one at a time, A-B-A (intact passes → neutered fails → restored passes) | See `ADR-0055`. Every guard is load-bearing; the per-guard results are in the ADR and the commit message |
