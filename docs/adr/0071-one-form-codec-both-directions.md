# ADR-0071: One codec, both directions, because the rule is narrower than RFC 3986

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the form codec shared by the callback and the token endpoint).
- **Relates to:** `ADR-0070` (the callback decoder's encoding is the opposite of the request encoder's),
  `ADR-0060` (a URL's query is built from encoded parts), `P3-006a` (two values that must agree, with nothing
  holding both).

## Context

`ADR-0070` added the callback decoder and asserted that its encoding differs from the request query encoder's
in one character: a space is `+`, not `%20`. That is correct, and it is not the whole rule.

HTML 4.01 §17.13.4 — which RFC 6749 Appendix B points at — says:

> Control names and values are escaped. Space characters are replaced by `+', and then reserved characters are
> escaped as described in [RFC1738], section 2.2: **Non-alphanumeric characters are replaced by `%HH'**, a
> percent sign and two hexadecimal digits representing the ASCII code of the character.

So the alphabet that survives unescaped is **alphanumerics only**. RFC 3986's unreserved set also keeps
`-`, `.`, `_`, `~`, and `crate::google::request::percent_encode` leaves them alone. A form component must escape
all four.

The decoder accepted them either way, so this was invisible in round one — a decoder that treats `~` as `~` is
correct, because the rule applies to producing the encoding. It becomes visible the moment an encoder exists,
and an encoder is needed by the token endpoint's form `POST` body (`P5-005`'s "connection setup", still
unbuilt). Writing it inside `token.rs` would have put a second copy of the one-character rule where the
callback already had one.

## Decision

Add `crate::form`, a single module holding both directions:

- `encode_component` / `decode_component` — alphanumerics pass through, a space is `+`, everything else is
  `%HH` with uppercase hex;
- `encode_body(&[(String, String)])` — the ordered pairs joined with `&`, **encoding the names as well as the
  values**;
- `CONTENT_TYPE` and `SPACE_ENCODED` as named constants.

`authorization.rs`'s local `form_decode` is deleted and its call sites use the shared decoder, mapping the
codec's `FormError` into `AuthRefusal::CallbackMalformed` through one function.

The module is **public**, because the encoder has no caller until the token transport exists and a `pub` item
inside a private module is unreachable — the dead-code defect `P5-001` recorded and this crate hit once before
when `google.rs` was private.

## The three consequences, each asserted

1. **A space is `+`.** The character where this and RFC 3986 disagree, and the one a caller is most likely to
   get wrong by reaching for the request encoder.
2. **`~ - . _` are escaped** (`%7E %2D %2E %5F`). Escaping *more* than necessary is interoperable in both
   directions — a receiver decodes `%7E` to the same byte — while escaping less produces a value the server
   reads differently. A test asserts the two encoders produce **different** output for `"a b~c"`, so a future
   refactor that merged them fails.
3. **Uppercase hex**, matching the RFC 3986 encoder's casing so a reader comparing them need not decide whether
   the difference is meaningful.

A fourth thing is deliberately **not** done: HTML 4.01 also says "Line breaks are represented as `CR LF` pairs
(i.e., `%0D%0A')" — an instruction to normalise, which only works if the receiver reverses it. OAuth's own
parameters cannot carry a line break: RFC 6749 Appendix A constrains `code`, `state`, and `refresh_token` to
`VSCHAR` (`%x20-7E`) and `error`/`error_description` to `NQSCHAR`. So this module encodes a line break
**faithfully** rather than rewriting it; the caller's validation should refuse it, and a codec that altered the
bytes would make that refusal unobservable.

## A defect the tests caught, in the test

The first `encode_body` assertion expected `"grant_type=authorization_code&code=a+b"` — with the **names
unescaped** — and the code produced `"grant%5Ftype=authorization%5Fcode"`. The code was right: §17.13.4 says
"control names **and** values". There is now a named test for a name containing `&` and `=` (`"a&b=c"` →
`"a%26b%3Dc"`) that fails if only values are encoded, and the body test's comment records the correction. This
is the same class as `ADR-0070`'s wrong premise: **the reasonable-looking expectation is the one to check.**

## Consequences

- One function holds the rule, so the callback's decoder and the token endpoint's encoder cannot drift.
- Two guards falsified A-B-A with compiling mutants: the space rule (detected by three tests, and the body test
  printed `left: "code=a b", right: "code=a+b"`) and the name escaping (detected by two, including
  `left: "a&b=c=v"`).
- `authorization.rs`'s local copy is gone, so the crate holds one form decoder rather than two.
- **Still not reachable end to end.** The encoder has no caller: the token endpoint's form `POST` transport is
  unbuilt, so `encode_body` is exercised only by its tests. That is why the module is exported rather than
  private, and the limit is recorded in both `TODO.md` and the research record.

## Alternatives considered

- **Keep the decoder local and write the encoder in `token.rs`.** Rejected: two implementations of a rule whose
  whole content is one character.
- **Reuse `percent_encode` with a flag.** Rejected: two rules behind one name is how the wrong one gets called,
  and the difference is not a parameter but a different specification (RFC 3986 §6.2.2.1 vs HTML 4.01 §17.13.4).
- **Normalise line breaks as HTML 4.01 describes.** Rejected: it rewrites a value the caller should refuse, and
  no OAuth parameter can carry a line break anyway.
