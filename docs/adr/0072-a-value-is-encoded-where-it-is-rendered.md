# ADR-0072: A value is encoded where it is rendered, never where it is stored

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the token exchange's request body).
- **Relates to:** `ADR-0071` (one form codec, both directions), `ADR-0070` (the callback decoder is not the
  request encoder), `ADR-0060` (a URL's query is built from encoded parts).

## Context

`ADR-0071` added `form::encode_body`, which escapes every parameter name and value it is given. The token
exchange already had parameter lists, and they were already escaped:

```rust
fn parameters(&self) -> Vec<(String, String)> {
    vec![
        (CLIENT_ID_PARAMETER.to_owned(), crate::google::request::percent_encode(&self.client_id)),
        (REDIRECT_URI_PARAMETER.to_owned(), crate::google::request::percent_encode(&self.redirect_uri)),
    ]
}
```

with a test asserting `redirect_uri == "http%3A%2F%2F127.0.0.1%2F"`.

Handing that list to `encode_body` escapes it **twice**: `http://127.0.0.1/` →
`http%253A%252F%252F127.0.0.1%252F`. Google answers a double-encoded `redirect_uri` with
`redirect_uri_mismatch`, and that error names client registration rather than the encoding — a reader would
hunt the wrong thing, and the actual fault is one layer away from the symptom.

The defect was **latent**: the body was never built, so the wrong-encoding path had no caller. It is the same
class as `ADR-0068` (a requirement with no implementation) and `ADR-0069` (a joint nobody drives) — but here the
two halves are the *producer* and the *renderer* of one value, and only a pipeline test covers them.

## Decision

`ExchangeIdentity::parameters()` returns **raw values**. The encoding happens in exactly one layer: whatever
renders the bytes.

Two accessors are added so the rendering layer is reachable and testable without a socket:

```rust
pub fn body(parameters: &[(String, String)]) -> String      // → form::encode_body
pub const fn content_type() -> &'static str                 // → form::CONTENT_TYPE
pub fn endpoint(&self) -> &'static str                      // → GoogleConnector::token_endpoint()
```

The endpoint is taken from the manifest rather than restated, which is how Google's own unusual detail is
preserved: `P5-004` recorded that the consent screen and the token exchange are on **different hosts**
(`accounts.google.com` and `oauth2.googleapis.com`), precisely the thing a reader might "tidy" into one constant.

## The tests, and the one that was reversed

`every_parameter_value_is_percent_encoded` asserted the pre-encoded form and was **removed**, because its
subject no longer exists. It is replaced by:

- `the_identity_holds_raw_values_and_the_encoding_happens_once` — the raw values, **and** that
  `body(&identity.public_parameters())` escapes them exactly once (`%3A%2F%2F`, never `%253A`), with an
  assertion that the rendered body contains **no `%25` at all**;
- `the_exchange_body_escapes_each_value_exactly_once` — the whole exchange pipeline, so a `/` in a code becomes
  `%2F` once;
- `the_identity_names_the_endpoint_the_manifest_declares`.

The `%25` assertion is the general form: a `%25` anywhere in a rendered form body means some value was escaped
where it was stored rather than where it was rendered.

## Consequences

- The exchange's body is correct the first time a transport builds it, rather than producing
  `redirect_uri_mismatch` when the transport slice lands.
- `public_parameters()` and `parameters()` have **raw** values, so a third consumer cannot inherit the trap. The
  change was safe: no call site outside this module existed.
- The old defect was restored as a mutant and detected by **two** tests, one of which printed the whole
  double-encoded body — `redirect%5Furi=http%253A%252F%252F127%2E0%2E0%2E1%252F` — so the failure names its own
  cause.
- **A latent defect was found by building the thing that would have exposed it.** The encoder had no caller
  until `ADR-0071`; the moment one existed, the two layers disagreed. That is an argument for writing the
  renderer before the transport rather than with it.

## Alternatives considered

- **Make `encode_body` detect already-escaped values.** Rejected: unrepresentable — an escaped `%2F` and a
  literal `%2F` are the same text, so a heuristic would either double-escape or under-escape depending on the
  input, and the rule would be a guess where a layering decision is available.
- **Have `encode_body` not encode names or values, leaving it to callers.** Rejected: that is how a caller
  omits the encoding entirely, and `ADR-0071` already established that the renderer owns it.
- **Keep the pre-encoding and have the body builder skip it.** Rejected: two rules about who encodes, which is
  the defect rather than the fix.
