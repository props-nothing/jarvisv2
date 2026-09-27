# ADR-0073: A retry-safety variant needs a producer, and the token exchange is it

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the token exchange over a transport).
- **Relates to:** `ADR-0072` (a value is encoded where it is rendered), `ADR-0071` (one form codec),
  `ADR-0069` (two tested halves do not test the seam), `ADR-0068` (a port is enforced by an implementation
  that exists), `P5-001` (a variant nothing constructs is dead code wearing a contract).

## Context

`token::TokenRequestOutcome` separates four cases with its own doc calling the distinction consequential:

- `NeverSent` — safe to retry, nothing reached the provider;
- `SentAnswerUnknown` — **not** safe, because RFC 9700 §4.2.4 makes a retry of a lost-answer request able to
  get `invalid_grant` *and destroy a working grant the first attempt issued*;
- `Answered` / `Refused`.

**Nothing produced `NeverSent` or `SentAnswerUnknown`.** Every construction site was in `token_tests.rs`. The
type was documentation of a capability rather than a capability — the defect `P5-001` records, in the form
`ADR-0067` also met: a variant that reads as a live condition while no code path reaches it.

There was a second reason it stayed unreachable. The exchange is a form `POST`, and
`transport::HttpMethod` deliberately has one variant, `Get`, so the port could not express the method — meaning
the type had neither a producer nor the *ability* to have one.

## Decision

Add the pieces the exchange needs, and the exchange itself:

1. **`request::FormRequest`** — a `POST` whose body is an already-rendered form, with its own `content_type`.
   A separate type from `HttpRequest` rather than a variant of it, because the two have opposite credential
   boundaries: `HttpRequest` exists so it *cannot* hold a credential in its URL (`ADR-0060`), while a token
   request's body **is** the credential-bearing text. A union would either give `HttpRequest` a field a
   credential goes in or force every caller to prove which kind it holds.
   - Its `Debug` is hand-written to `[REDACTED]` plus the character count, and it has **no `Display`** — the
     `ADR-0061` rule, since a derived `Debug` would render the body through any `{:?}`.
   - It takes the **rendered** body, so it cannot encode and therefore cannot double-encode (`ADR-0072`).

2. **`transport::GoogleTransport::send_form`** — a second port method rather than a third `HttpMethod`.
   `HttpMethod`'s doc says adding a write is `P5-009`'s decision; this is not that, it is the token endpoint's
   *framing*, which authenticates by its body's `client_id` and has no bearer token. One `send` with two
   disjoint modes would give the read path a body it does not have and the token path a credential parameter it
   does not use.

3. **`token::exchange`** — the request/response join, whose one line maps a transport failure by
   `may_have_reached_the_provider` into `SentAnswerUnknown` or `NeverSent`.

4. **`TransportFailure::reason`** — a `const fn` returning a bounded `&'static str`, because `NeverSent` holds
   a static reason and formatting the `Display` would allocate and lose the lifetime. Its doc makes **certainty
   part of the contract**: an ambiguous variant must not return a reassuring sentence, and a test asserts the
   pairing against the predicate rather than against the wording.

### `reqwest`'s `form` feature is deliberately not enabled

`.form()` exists but is behind a feature this workspace does not enable, and enabling it would add the
dependency's form encoder to the tree beside `crate::form` — two implementations of the one character
`ADR-0071`/`ADR-0072` exist to keep single, disagreeing invisibly because both produce a plausible body. So the
request is built with `.body(rendered)` and an explicit `Content-Type`.

## The tests, and the shape of the assertion

`token_exchange_tests.rs` drives a `ScriptedForm` transport that **records** what it received, so the
assertions are about a real exchange: the endpoint from the identity, the media type, and a body with no `%25`
(no double-encoding).

The retry-safety cases are asserted **against `may_have_reached_the_provider`** rather than against a
hand-written list of which failures are ambiguous. A new variant therefore cannot be classified by omission:
the fixture fails first if the predicate disagrees with the expectation.

An unreadable body is an `Err`, not `Refused`. Folding it into a refusal would send a user to a consent screen
when the real fault is a body this client cannot parse — and the two are different things: "the provider
answered no" versus "I could not read the answer".

## Consequences

- `SentAnswerUnknown` and `NeverSent` are reachable from a real transport failure, so the four-way split is a
  capability rather than a comment.
- Two guards falsified A-B-A with compiling mutants: collapsing the ambiguity (detected with
  `Send may have been written, so a retry could repeat an effect` / `left: NeverSent` / `right:
  SentAnswerUnknown`) and turning an unreadable body into a refusal.
- Both test doubles that implement the port gained a `send_form` arm, each refusing a call the other path would
  make — so a wrong path is a test failure rather than a silent success.
- **Still not a live path.** The exchange has no caller: nothing constructs a `FormRequest` outside a test,
  because no composition root builds the connector. The endpoint, parameters, body, request type, port and
  answer reader are now all present and tested, so a live call needs credentials and a composition root rather
  than more code here.

## Alternatives considered

- **Add `Post` to `HttpMethod`.** Rejected: it makes the read path expressible as a write, and the token request
  needs no bearer token — so `send` would take a parameter the caller must invent.
- **Put the body in `HttpRequest`.** Rejected: it undoes `ADR-0060`, and its `%`-encoded query would collide
  conceptually with a form body whose escaping differs.
- **Let `exchange` return `Err` for a transport failure.** Rejected: that is exactly the information loss the
  four-way type exists to prevent, and it would make the ambiguity unrepresentable at the only call site.
