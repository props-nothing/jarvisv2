# ADR-0060: A URL's query is built from encoded parts, and the type has no field for a credential

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (Google request construction).
- **Relates to:** `ADR-0058` (a provider decision belongs in the crate that does not own a socket).

## Context

`ADR-0058` put Google's *decisions* in `jarvis-connectors` and left the transport as a named later step. The
next thing that can be built without a socket is the **request**: a method, a URL, and parameters. Building it
surfaced two rules that had been implicit.

### Implicit rule 1: a query value is model-chosen text

A Gmail search query comes from a model. Interpolated into `?q=…` it can contain `&`, `=`, `#`, `%`, or `?` —
and each of those changes the *request* rather than the query:

- `&` starts a new parameter, so `is:unread&maxResults=999` replaces the connector's own bound with the
  model's. The request that arrives is one the connector never intended and its own `maxResults` bound is
  gone.
- `=` makes the provider read part of the text as a parameter assignment.
- `#` ends the query, so everything after it is dropped.
- `%` starts an escape sequence of the value's own choosing.
- `?` starts a second query string.

None of these are hypothetical for a search box, and all of them are silent: the request succeeds and returns
something.

### Implicit rule 2: `?access_token=` exists, and Google says why not to use it

Google's own native-app page offers `?access_token=<token>` as an alternative to the `Authorization` header
and adds that "query strings tend to be visible in server logs". A URL is the part of a request that appears
in a proxy's access log, a reverse proxy's error page, and a diagnostics line. So the parameter is a supported
way to do the one thing that leaks a credential into every artifact a debugging session produces.

## Decision

**1. Every query value is percent-encoded per RFC 3986, and that is a security control rather than tidiness.**

`percent_encode` escapes everything outside the unreserved set (`ALPHA / DIGIT / - . _ ~`) as `%XX` with
**uppercase** hex, which §6.2.2.1 normalisation requires — lowercase is equivalent but not canonical, so a
recorded fixture would differ from what the provider echoes.

**2. A space is `%20` and a literal `+` is `%2B`, because the form-urlencoded convention is ambiguous here.**

`application/x-www-form-urlencoded` writes a space as `+`. Encoding `+` as `%2B` and a space as `%20` removes
the ambiguity in both directions: a receiver that decoded `+` as a space would misread a value containing a
literal one, and Gmail's search syntax has characters a caller might reasonably pass through. `%20` is also
correct in **every** query position, whereas `+`-as-space is only correct when the receiver applies that rule.

**3. `HttpRequest` has no field a credential could go in.**

A method, a URL, ordered parameters, and an `Accept` value. There is no header map, no token, and **no body** —
all three declared operations are `GET`s, so a body field would be a shape nothing uses, and the first caller
to put arguments in a body would be writing a request Google rejects rather than one this module refused. The
absence is the control, the same technique as `jarvis_core::SecretRef` having no `value` field: a rule saying
"do not put a token here" is weaker than a type that cannot hold one. The header's **name** and scheme are
constants (`AUTHORIZATION_HEADER`, `BEARER_SCHEME`) so a transport knows where a credential belongs, and the
value is not in this crate.

**4. Parameters are an ordered `Vec`, not a map.**

A map's iteration order would make the same logical request produce different URLs on different runs, which
makes a recorded fixture unusable and a signature over the request impossible.

**5. A parameter is omitted when unset, never sent empty.**

`name=` is a different request from omitting the parameter, and for `pageToken` the difference is between "the
last page" and "a token Google rejects". An **empty Gmail query** is the exception and *is* sent, because the
schema's own description says an omitted query means "the newest messages" — so the empty string is a
legitimate value rather than an absent one.

**6. `format=raw` is not representable.**

`MessageFormat` is a three-variant enum with no `Raw`. `format=raw` returns the unparsed MIME message including
attachments, nothing in this connector parses it, and the tool's input schema already omits it. A type that
cannot express the value is stronger than a check that refuses it.

**7. `status` is checked before the body is parsed.**

Every parse function refuses a non-200 first. The failure mode this prevents is specific: an error document
parsed as a page reports "no results", and a caller cannot then tell a successful empty mailbox from a refused
request. The refusal's reason names `client::classify` as where the outcome actually belongs, so a reader is
sent to the right layer.

**8. `nextSyncToken` and `nextPageToken` are separate fields and are not interchangeable.**

Google's sync guide says the sync token "is present only on the very last page", while the page token continues
the current walk. A caller that stored the page token as a cursor would store something that expires when the
walk ends. So `CalendarPage` carries both, and a test asserts a fixture with both values keeps them distinct.

**9. `Display` reports the method, the path, and parameter NAMES, never a value.**

A rendering reaches a log line and a value is model-chosen text. A parameter's name is this module's own
constant; its value is not. This is what `HttpRequest::url()` (path only) and `url_with_query()` (the full
target) are split for: a diagnostic can render the shape while the transport builds the target.

## Consequences

- **The injection is impossible rather than discouraged**, and falsified: a mutant that admitted `&`, `=`,
  `#` and `%` into the unreserved set made the injection test fail.
- **A credential has nowhere to go in a URL**, which is asserted on every request the module can build, by
  searching each rendered URL for `access_token`, `token=`, `key=`, and `api_key`, and by checking that every
  parameter name is one of this module's constants.
- **`maxResults` is bounded per API** — 500 for Gmail, 2 500 for Calendar — because a single shared bound would
  be wrong for one of them. Zero is refused rather than read as "unlimited", and the value at each cap is
  accepted so neither bound is unreachable.
- **The query length and identifier length are JARVIS bounds, not provider figures**, and the docs say so.
  Gmail publishes no query length limit, so presenting one as documented would be the defect
  `RateLimitEvidence` exists to prevent.
- **A resource identifier is encoded as a path segment**, which matters for a different reason than a query
  value: a `/` in an identifier would change which resource is addressed. Falsified by a fixture containing
  `msg/../other`.

## Limits

- **No request has been sent, and nothing performs one.** There is no transport binding, so `HttpRequest` has
  no caller in production code. The URLs are asserted against the research record's API bases and paths;
  whether Google accepts them is established only by a live smoke test that does not exist.
- **The response parsers have never seen a real response.** Every fixture is built from the research record, so
  the tests prove the code reads the *record's* shape. Gmail's `messages.list` returns a full `Message`
  resource per entry and this module reads only `id` from each, which is a deliberate narrowing — but it means
  the parser ignores fields nobody has checked exist or are absent.
- **`parse_id_page` accepts a body with no `messages` array as an empty page.** That is correct for
  `{"resultSizeEstimate": 0}` and it also means a body of an entirely different shape parses successfully with
  no results. The status check catches an error document, and a *successful* wrong-shaped document is not
  distinguishable — recorded rather than fixed, because the fix would be a stricter schema whose contents
  nobody has observed.
- **No retry, no pacing, and no budget accounting happens here.** `client::classify` produces a decision and
  this module produces a request; nothing connects them. A request that should be delayed is delayed by
  nobody.
- **The `Accept` header is stated and no other header is**, so a transport still has to supply `Authorization`
  and anything else Google requires. That is a gap in coverage rather than a decision: the headers a live call
  needs have not been observed.
