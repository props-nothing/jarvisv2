# ADR-0070: The callback decoder's encoding is the opposite of the request encoder's

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the OAuth loopback callback).
- **Relates to:** `ADR-0060` (a URL's query is built from encoded parts), `ADR-0055` (the authorization
  transaction is consumable once), `ADR-0068` (a transport port is only enforced by an implementation that
  exists), `ADR-0069` (two tested halves do not test the seam between them).

## Context

`Callback` — the value the loopback listener hands to `AuthorizationTransaction::consume` — carried a doc that
said its parameters "arrive **already decoded**, because this crate has no URL decoder and duplicating one would
put a second percent-decoding implementation in the path that all four checks read."

That division was reasonable, and it left the decode **unimplemented**. Every `Callback` in the tree was
assembled by hand in a test, so no code in this repository read a request target from a socket:
`ADR-0068`'s shape (a stated requirement with no implementation) and `ADR-0069`'s (a joint nobody drives), one
layer further out — the *listener* had no reader.

## Decision

Add `Callback::from_request_target`, plus a `FormParameters` type and a `form_decode` function.

**The decoding is `application/x-www-form-urlencoded`, and it is the opposite of the request side's encoding.**
RFC 6749 §4.1.2 says a server adds the response parameters "to the query component of the redirection URI using
the `application/x-www-form-urlencoded` format, per Appendix B". In that format a space is **`+`**. On the
request side, `ADR-0060`'s `percent_encode` encodes a space as `%20` **and a literal `+` as `%2B`**, on purpose,
because RFC 3986 has no form semantics and `+` is used literally by Gmail's search syntax.

So the same crate must hold both rules, and they disagree about one character:

| | request (building) | callback (reading) |
| --- | --- | --- |
| space | `%20` | `+` |
| literal `+` | `%2B` | `%2B` |

Consequences: a `state` of `a b` arrives as `state=a+b`; a decoder that kept `+` would compare `a+b` against
`a b` and refuse a legitimate response — **and the failure would present as a `StateMismatch`, i.e. as an attack,
while the cause is one character of decoding.** That is the worst shape a security refusal can take.

## The variant split, which a test forced

`AuthRefusal` gained two variants rather than one:

- `CallbackMalformed { reason }` — the bytes could not be read: a truncated or non-hex escape, bytes that are
  not valid UTF-8 after decoding, or a target that does not name a loopback redirect;
- `ParameterRepeated` — a name appeared twice.

They were one variant in the first draft, and the codetable test failed on `indicates_forgery` (`left: true,
right: false`) because the single variant had to answer for both. That is exactly the "two values standing for
more than two situations" defect this repository keeps recording: **a truncated escape is a defect in the
listener and belongs in a developer's lap, while a repeated parameter is the shape an appended value takes and
is a possible forgery.** `indicates_forgery` is now `true` only for the four mismatch/repeat variants.

Also refused, deliberately: a **repeated parameter is not last-wins**. RFC 6749 §3.1 and §3.2 both require
parameters "MUST NOT be included more than once"; taking the last value is precisely how a `state` check is
defeated, since the server's own value comes first and an attacker's appended one second.

A malformed escape or invalid UTF-8 is **refused, not lossily decoded**. Appendix B says a parsed value "need[s]
to be treated as octet sequences, to be decoded using the UTF-8 character encoding scheme" — a lossy decode
turns a corrupted `state` into a *different* string that merely fails to match, hiding a transport fault behind
a security refusal.

## Consequences

- The listener has a reader, so the decode step is written and tested rather than assumed.
- Two guards were falsified A-B-A with compiling mutants: `+` → a literal (detected with
  `left: Some("a+b"), right: Some("a b")`) and permitting a repeat (detected by `expected a refusal`).
- The `received_on` value is recovered **portless**, because an origin-form target carries no port — the `Host`
  header holds it and this function is not given one. `consume` still compares it against the transaction's
  ported registration through `matches_except_port`, so the port check is not lost; it is done where both
  values exist. A test premise claiming a non-loopback target would be refused was **wrong** (an absolute-form
  string parses as a path, so it is accepted) and was replaced by the accurate one.
- **Still not a live path.** No socket is bound; nothing in this crate runs a listener, so a callback has still
  never arrived over HTTP. What is closed is that the decode exists and is tested.

## Alternatives considered

- **Reuse `percent_encode`'s inverse.** Rejected: the two encodings differ in a way that matters (`+`), so a
  shared codec would have to take a mode flag, and the wrong flag would silently refuse legitimate responses.
- **Keep the decode in the listener (outside this crate).** Rejected: the security checks in `consume` read
  these values, and a decoder outside the tested code is where the `+` mistake would live unobserved.
- **One `CallbackUnparsable` variant.** Rejected by the failing test — see above.
