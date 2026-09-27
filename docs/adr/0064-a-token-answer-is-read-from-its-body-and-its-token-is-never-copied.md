# ADR-0064: A token endpoint's answer is read from its body, and the token it grants is never copied into a value

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google connection's token exchange).
- **Relates to:** `ADR-0060` (a URL's query is built from encoded parts), `ADR-0061` (a credential cannot be
  rendered or serialized), `ADR-0062` (an unanswered request is classified by whether it may have reached the
  provider), `ADR-0055` (`TokenSet` has no access-token field).

## Context

`P5-005` is "Implement Google connection setup and Gmail/Calendar read tools". The **connection setup** half was
the least built part: the crate had `AuthFlow` (authorization request, PKCE, redirect) and the token *types*
(`TokenResponse`, `TokenSet`, `TokenEndpointFailure`, `RefreshExchange`) — and nothing that turns an
authorization code into a grant. So a connector could start a flow and could classify a refresh, and could not
complete a first authorization.

Two things about that step are easy to get backwards, and both were live hazards in this codebase:

**First, which half of an HTTP response decides whether a token request was refused.** The rest of this crate
reads refusals from the **status** — `client::classify` switches on the code, and `TransportResponse::is_success`
is `status == 200`. OAuth is the opposite. RFC 6749 §5.1 puts the token parameters in a *successful* response
body, and §5.2 makes a failure an `error` parameter with those parameters omitted. So the presence of `error`
is what makes an answer a refusal, and a `400` **without** one is a proxy's page or a misrouted request. Reading
the status first in this module would have reported "the server refused" and attributed a decision to a server
that never made one.

**Second, whether reading an `access_token` out of a response body is safe.** `ADR-0061` built `AccessToken` so
the material cannot be rendered, serialized, or reached except through a closure, and `TokenSet` has no field
for it. **The first draft of this module read `access_token` into an owned `String` anyway** — in the
implementation of `parse_answer` — and then wrote a comment saying it was dropped. It satisfies the letter of
the rule and breaks its purpose: an owned copy existed, with a lifetime, in a function whose other outputs are
`Debug`-printed.

## Decision

**1. The response body decides whether an answer is a refusal; the status decides only whether the provider is unwell.**

`parse_answer` checks the **presence** of `error` first. If it is present, the answer is
`TokenEndpointAnswer::Refused`, carrying the code and the description. The HTTP status contributes exactly one
bit: `5xx` marks the refusal transient. That bit is there because RFC 6749 §5.2's codes describe the *request*
and cannot say anything about the server's health — that is the transport's observation.

**2. A refusal is a variant, not an error.**

`TokenRequestError` carries the three ways an answer could not be *used* — an unusable parameter, an unreadable
body, and a grant this platform will not accept. A refusal is `TokenEndpointAnswer::Refused`. So the type
mirrors `jarvis-tools`' split between "I could not run" and "here is what happened when I did", and
`TokenEndpointAnswer`'s two accessors are **disjoint**: exactly one of `response()` and `failure()` is `Some`,
which is asserted.

**3. An unreadable body is not a refusal.**

A `400` whose body is HTML, or JSON with neither a token nor an error, is `TokenRequestError::Body`. Reporting it
as `Refused` would invent a provider decision; reporting it as a *transport* failure would claim the provider
was unreachable when it answered. Three outcomes, three types.

**4. A grant that cannot be used is an error, not a refusal — and the check is duplicated deliberately.**

A `token_type` other than `Bearer`, or an `expires_in` beyond `MAX_ACCESS_TOKEN_SECONDS`, means the exchange
*completed* and granted something this platform will not send. It is `TokenRequestError::UnusableGrant` rather
than `Refused`, because saying the provider declined would be false. The check uses the same predicates
`TokenSet::from_response` uses **and both are tested on the same values**, so they cannot disagree — a caller
that only wants to know whether the exchange worked does not have to know the check lives in another module.

**5. `parse_answer` never copies the access token.**

It asks whether a non-empty `access_token` is **present** (`.is_some_and`, producing a `bool`) and reports that
presence through `TokenResponse`. The bytes are never bound to a named value. `Granted` has **no field for
them**: a field holding the text would be a second, unredacted copy of the credential in a value the rest of
this crate `Debug`-prints, which is what `ADR-0061` exists to prevent. A test renders a granted answer and
asserts neither the access token nor the refresh token appears.

**6. The access token is `String`-free; the refresh token is wrapped immediately.**

The refresh material *must* leave the module usable, so it is wrapped in `token::Secret` inside `parse_answer` —
hand-written `Debug` to `[REDACTED]` plus a character count, and **no `Serialize`**. The window in which the
plaintext exists as a bare `String` is one statement.

**7. `Secret` refuses a paste mistake at construction, and the refusals are ordered by actionability.**

Empty, oversized, whitespace-containing, and control-containing are four separate reasons. Internal whitespace is
never legitimate in a bearer string, and the specific case is a **trailing newline from a paste**: it reaches
the provider as a different string and surfaces as a generic auth failure, which sends a reader to debug the
credential's *validity* instead of its *shape*. `ADR-0061` records the same ordering decision for a pasted
`Authorization` header.

**8. The PKCE verifier is checked against RFC 7636's bounds and its alphabet.**

43–128 characters, over `ALPHA / DIGIT / "-" / "." / "_" / "~"`. A verifier outside the alphabet produces a
challenge mismatch that looks like a PKCE bug rather than malformed input. An **empty** verifier is refused
earlier, by the credential shape — which is the correct ordering, because an empty value is a paste mistake
rather than a bound violation, and a test records that.

**9. The request is a parameter list, not an `HttpRequest`.**

`HttpRequest` is a `GET` with query parameters and no field for a credential, by design. A token request is a
`POST` with an `application/x-www-form-urlencoded` body and the credential *inside* it. Forcing one into the
other would either give `HttpRequest` a body and a credential field — undoing `ADR-0060` — or require a
credential-bearing type to be renderable. So the module returns `Vec<(String, String)>`: no `Display`, no
`Serialize`, and the caller owns the wire.

**10. There is no `client_secret` field anywhere.**

`P5-005`'s manifest declares empty `secret_fields`, because a PKCE public client identifies itself by an
identifier that is not secret, and Google documents `client_secret` as optional on both exchanges. This is not
an optional parameter a caller is trusted to leave unset — it is **no field to put it in**, so a connector that
sends one is unrepresentable. The parameter set is asserted as an exact **set**, so an added parameter is a
failing test.

**11. The transient check precedes the error code when classifying a refresh, and both halves are asserted.**

A provider behind a proxy can answer a `503` through the protocol's own error channel — a real shape — so reading
`invalid_grant` first would send a user to a consent screen during an outage. The test asserts the ordering
*and* the control: the same code without the outage **is** the user's problem.

**12. Four guards were falsified with compiling mutants.**

Ignoring the `error` parameter (8 tests detected), letting the wrong branch win the transient ordering (2),
never reporting a rotation (2), and dropping the credential-shape whitespace check (2).

## Consequences

- **The body-authority rule is now stated where the status-authority rule lives.** The asymmetry is a real
  property of the two protocols, not an inconsistency, and both are recorded so a reader who finds one does not
  "fix" the other.
- **The connection-setup half of `P5-005` is no longer empty.** A caller can build both request bodies and read
  either answer, which is what the flow was missing.
- **`ADR-0061`'s boundary survived contact with a response body** — the case most likely to break it, because a
  response body is exactly where a credential arrives as text.
- **A self-caught error is recorded rather than quietly fixed.** The first draft copied the access token into a
  `String` "and dropped it"; the rule is about copies, not about lifetimes, so the draft was replaced.

## Limits

- **No request is sent, and no token has ever been obtained.** There is no transport for a form `POST`, so
  nothing in this slice performs an exchange. Every body in the tests is hand-built from the reference pages, and
  the fixtures under `tests/fixtures/google/` do **not** yet include a token response.
- **The `access_token` never reaches a value, which means the caller cannot get it from this module.** That is
  deliberate, and it has a cost worth naming: a caller must read the material from the response body itself,
  which means the bytes exist outside this module's boundary and this module cannot enforce anything about them
  there.
- **No ID-token verification.** The manifest requests `openid`, Google returns an `id_token`, and this module
  records only that one arrived. The signature check against `jwks_uri` is still unbuilt, so the `nonce` that
  `P5-002` carries is still unvalidated.
- **No DPoP.** Google supports it, recommends it, and it needs a non-exportable signing key; sending a
  `DPoP` header is a decision with a key-management obligation, recorded in the research record rather than
  implemented.
- **`redirect_uri` is not validated against the loopback rules here.** `ExchangeIdentity` accepts any string,
  because it is the *flow* that owns the registered value; a caller could therefore build a request with a
  redirect the flow would refuse.
- **The parameter list is not tested as a body.** Nothing asserts that `Vec<(String, String)>` serializes to a
  form body the way a transport would, because no transport exists — so the encoding is asserted per value and
  the joining is not.
- **The `client_id` is not validated beyond being non-empty.** A client identifier that is a pasted URL or a
  project number is accepted here and would fail at the provider, which is the same class of gap `ADR-0061`
  closed for the credential field and not for the client identifier.
