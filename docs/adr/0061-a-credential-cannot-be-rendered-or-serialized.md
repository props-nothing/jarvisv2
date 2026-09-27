# ADR-0061: A credential is a type that cannot be rendered, serialized, or reached by accident

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google access-token boundary).
- **Relates to:** `ADR-0060` (a URL's query is built from encoded parts), `ADR-0055` (`TokenSet` has no
  access-token field).

## Context

`ADR-0060` made a credential unrepresentable **in a URL** by giving `HttpRequest` no field to hold one. That
closes one route and not the ones that leak credentials in practice. A token reaches an artifact through an
*impl*, not through a data structure:

- **`Debug`** — `tracing::debug!("calling {} {:?}", url, headers)` renders whatever the struct holds. A
  derived `Debug` on a request type is a credential leak with a one-word trigger.
- **`Serialize`** — anything that puts an authenticated request into a durable row, a wire DTO, or a
  diagnostics document.
- **`Display`** — an error message. A transport that wraps a failure in `format!("{request:?}")` leaks it, and
  so does an error variant that holds a value.
- **A struct field** — a `String` argument passed to a send function is `Debug`-printable by everyone who
  holds it, including a caller that only meant to log its own state.

`jarvis_core::SecretRef` already takes one step in this direction: no `Serialize`, a hand-written `Debug` that
redacts the locator, and a `compile_fail` doctest. But `SecretRef` is *metadata that locates* a secret and
deliberately holds no value. A transport needs the value itself.

## Decision

**1. `AccessToken` implements neither `Serialize` nor a derived `Debug`.**

`Debug` is hand-written to `AccessToken { value: [REDACTED], chars: N }`. The length is reported because it is
not the value and it is what a diagnostic needs: two credentials of different lengths are visibly different
credentials.

**2. The absence of `Serialize` is asserted, not described.**

The type carries a `compile_fail` doctest that serializes it. A later `#[derive(Serialize)]` therefore breaks
the build. **And the doctest was falsified**: replacing the serialization line with a call that *should*
compile made the doctest fail, which is what proves it fails for the serialization rather than for a bad import
path — the distinction a `compile_fail` test cannot make on its own.

**3. The bytes are reachable through exactly one accessor, and its name is the warning.**

`with_exposed(|token| …)` takes a **closure** rather than returning a `&str`, so the borrow cannot outlive the
call and a caller cannot move the material somewhere a later `Debug` could reach it. The closure receives only
the text, not the `AccessToken`, so reaching the bytes requires writing the word `exposed`.

**4. The header is rendered by the token, not by the transport.**

`authorization_header_value()` is the single place the bytes and the scheme meet. A transport that built
`"Bearer " + value` itself would be a second implementation, which is where a missing space or a doubled
scheme comes from — and the doubled scheme is a real shape, because a pasted value may already carry it.

**5. A pasted `Authorization` header reports removing the scheme, not the whitespace.**

`Bearer <token>` contains both a scheme and a space. The scheme check runs **first** so the message says what
to do; checking whitespace first would report "contains whitespace", which is true, useless, and sends a reader
hunting an invisible character. `P5-004` records the same ordering decision for an API key, and the two
checks are separate variants because the remedies differ.

**6. A length floor makes a paste mistake surface where it was made.**

`MIN_ACCESS_TOKEN_CHARS = 20`. A client identifier or a project number is shorter, so the common mistake is
refused at construction rather than at the provider, where a generic auth error sends a reader to debug the
credential's *validity* instead of its *shape*.

**7. The type may name which credential it came from without naming what it is.**

`origin: Option<SecretRef>` is the one field a diagnostic may print: "the stored refresh exchange for this
account failed" rather than "some token is wrong". A `SecretRef` is metadata, and its own `Debug` redacts its
locator — asserted, not assumed.

**8. Both modules state one header name and one scheme, and a test asserts they agree.**

`AUTHORIZATION_HEADER` and `BEARER_SCHEME` appear in the request module and here, because the request states
*where* a credential goes and this type states *what* goes there. Two constants for one fact is the "two values
that must agree, with nothing holding both" defect, so they are compared rather than trusted.

**9. Every refusal names the problem and none renders the value.**

An error about a credential reaches a log, so an error that printed the value it refused would leak the thing
it was protecting. The test asserts it for every variant at once, because a **new** variant is the case that
would forget.

## Consequences

- **A one-word mistake is no longer enough to leak a token.** Before this, a `#[derive(Debug)]` on any
  containing type, or one `{:?}` in a log statement, would print it. Now the compiler refuses the serialization
  and the rendering is redacted.
- **Reaching the material requires a visible act.** Every exposure site contains the word `exposed`, which is
  what a reviewer greps for and what a diff shows.
- **The `compile_fail` doctest is a build-time assertion**, so this control is enforced by `cargo test --doc`
  rather than by review. It is the second such assertion in the workspace, after `SecretRef`.
- **No dependency was added.** No `zeroize`, no `secrecy`: both are real options and neither is decided here —
  see the limits.

## Limits

- **Nothing holds an `AccessToken` in production code.** There is no transport binding, no token source, and no
  exchange, so the type has no caller outside its tests. It is a boundary waiting for the thing it protects.
- **The material is not zeroized on drop.** `zeroize` is not a dependency, and adding one is a decision rather
  than a detail — it would need a workspace-wide answer about which types zeroize, and a partial answer is worse
  than a recorded gap because it implies coverage. So the residual is: a token's bytes remain in freed memory
  until that memory is reused.
- **`String` reallocation can leave copies.** `authorization_header_value` allocates a new `String` holding the
  credential, so a `String`-returning API is itself a small widening of the exposure — the value is now in two
  buffers. Recorded rather than hidden: a transport could take a closure instead, and that is a shape to
  consider when the binding is written.
- **`Debug` on a `Vec<u8>` of the header would still print it**, because this type controls its own rendering
  and not the rendering of anything derived from it. The `authorization_header_value` result is an unprotected
  `String`, deliberately named to make that obvious.
- **The length floor is a heuristic, not a validity check.** A well-formed string of the right length proves
  nothing about whether it is a valid credential, and this type makes no attempt to check.
- **Nothing validates that a token is *for* Google.** A token for another provider would be accepted, because
  the type checks shape and not provenance — and provenance is not a property a string carries.
