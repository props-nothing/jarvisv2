# ADR-0062: An unanswered request is classified by whether it may have reached the provider

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google transport port and read operations).
- **Relates to:** `ADR-0058` (a provider decision belongs in the crate that does not own a socket), `ADR-0059`
  (a tool definition is derived from the connector's manifest), `ADR-0060` (a URL's query is built from encoded
  parts), `ADR-0061` (a credential cannot be rendered or serialized), `ADR-0019` (a tool call is a durable
  lifecycle).

## Context

`ADR-0058` deliberately kept this crate free of an HTTP stack, so every rule in it is a pure function of its
arguments and can be tested without a socket. That leaves one thing unimplemented: something must **perform** a
request. So this slice adds a port — a trait — and a layer that turns what the port observed into what an
adapter must report.

Two facts about that boundary are easy to get wrong in opposite directions.

**First, "the request failed" is not one condition.** The pipeline's outcome vocabulary (`jarvis-tools`'
`AdapterError`) already distinguishes `RefusedBeforeReaching` from `AmbiguousAfterReaching`, and a transport has
to decide which applies. A DNS failure, a refused connection, and a policy refusal all happen before anything is
written. A timeout and a broken connection *during* a write do not: the provider may have received the request
and acted on it. Reporting the second group as the first invites a retry, and for a non-idempotent effect that
retry is a **second** effect.

**Second, a provider's refusal is not a failure.** An HTTP `403` means the request was received and refused.
That is an answer, and it belongs in `AdapterError::ProviderRefused`'s territory — which is to say, it is a
`ToolCallResult` with a `Failed` outcome, not an error. Collapsing a non-2xx status into a transport error loses
the status and the machine-readable reason code, which is everything needed to decide what to do next.

A third case sits between them and is the one most likely to be flattened: a `200` whose body cannot be parsed.
The status proves the request was answered; the body says nothing about what it produced. That is neither a
confirmation nor a failure.

## Decision

**1. The port is a trait with an `async` method, and it names the two arguments separately.**

```rust
async fn send(&self, method: HttpMethod, request: &HttpRequest, token: &AccessToken)
    -> Result<TransportResponse, TransportFailure>;
```

`request` and `token` are separate parameters rather than one authenticated-request type, because `HttpRequest`
may be rendered (its `Display` prints the path and parameter *names*) and `AccessToken` may not. Merging them
would make a single `{:?}` leak the credential — the control `ADR-0061` built, undone by a convenient
signature.

**1a. The port is `async` because the interface it must satisfy is, and a synchronous first draft was wrong.**

`jarvis_tools::ToolExecutor::execute` is `async`, so an adapter binding this port must be `async`. An `async`
function calling a blocking one is not a style choice: it blocks a runtime worker for the whole round trip,
which is the failure `P2-007` records for a blocking read inside a stream. **The first draft of this port was
synchronous and was replaced**, because a synchronous port can never back a Google adapter — so the "defer the
async binding" reasoning that produced it was internally consistent and still produced an unreachable port.
`#[async_trait]` is used rather than native `async fn` in a trait so the trait stays **object safe**.

**2. The method is a closed `enum`, not a `&'static str`.**

All three declared operations are reads, so `HttpMethod` has one variant. `POST`, `PATCH`, `PUT`, and `DELETE`
are **absent rather than present-and-unused**: a variant nothing constructs is a method a reader assumes is
reachable. When a write arrives (`P5-009`) the enum grows and every `match` on it becomes a compile error, which
is what makes a new method a deliberate edit rather than a convenience.

**3. A transport failure is an enum whose variants encode *when* the failure happened.**

`Connect`, `Send`, `Body`, `Timeout`, `Refused { reason }`. `Send` exists separately from `Connect` precisely
because "the request was written and then the connection broke" is not "the request could not be sent" —
conflating them is how an ambiguous failure becomes a certain one.

**4. The ambiguity is a named predicate, and a test asserts the whole table.**

```rust
pub const fn may_have_reached_the_provider(self) -> bool
```

`false` for `Connect` and `Refused`; `true` for `Send`, `Timeout`, and `Body`. It is deliberately **not** named
`is_certain_nothing_happened`, because that reading invites a default of `true` in a `match`'s fallback arm and
a new variant would then silently become ambiguous-and-retryable. It is a `match` with no wildcard, so a new
variant is a compile error.

**5. The mapping is asserted with its consequence, not its variant.**

The test walks all five variants and asserts **both** the classification and the `AdapterError` it produces.
Asserting the class alone would still pass if the two error variants were swapped in a way that happened to
satisfy one assertion; the pair is what pins the direction that has a consequence.

**6. `AdapterError::ProviderRefused` is never produced by a transport failure.**

That variant means "the provider answered a refusal". A transport failure is by definition **no answer**, so
using it would state that a provider decided something when nothing was heard from it. A test asserts this for
all five variants at once, because a new variant is the case that would forget.

**7. A non-2xx status is a response, not a failure, and `is_success` is narrow.**

The port's contract states that a transport must **not** return an error for a non-2xx status. `is_success()` is
`status == 200` rather than the whole `2xx` class: both APIs answer `200` for every operation this connector
declares, and accepting the class would let a `204` with no body be read as a confirmed page.

**8. A `200` whose body cannot be read is `Unknown` — and is a *result*, not an error.**

The provider answered, so the adapter has something to report. `Unknown` refuses an automatic retry, which is
the honest consequence: the request was received and nothing is known about what it did.

**9. A refusal's reason carries the provider's reason *code*, never its prose.**

`GmailErrorReason::parse` matches the machine-readable `errors[].reason` exactly. A body that cannot be parsed
still yields a reason, because the **status** is a fact even when the body is not readable — the difference
between "we know little" and "we know nothing". The reason is truncated to this crate's own bound
(`MAX_FAILURE_REASON_CHARS`) rather than passed through, because `ToolOutcomeRecord::failed` would refuse an
oversized reason and thereby turn a provider refusal into an adapter defect.

**10. An argument fault never reaches the network, so it is `RefusedBeforeReaching`.**

A malformed argument or an unknown tool is refused before a socket exists, and `RefusedBeforeReaching` is the
accurate statement. An unknown tool is `NotImplemented` rather than a default, because a fallback would make a
mistyped tool name silently read a mailbox.

**11. The operation layer renders the tool's *declared* output, not the provider's resource.**

`read_output` emits `message_ids` / `event_ids` plus `next_page_token` — matching the tool's own JSON schema
(`ADR-0059`) rather than Gmail's `Message` type. A parser returning the provider's resource would make the
declared schema a fiction. `next_sync_token` is rendered **separately** from `next_page_token`, because the sync
token positions a future incremental sync while the page token continues the current walk; merging them would
store a cursor that expires with the walk.

**12. `GoogleReadTool` binds the pieces, and it implements `ToolExecutor`.**

Arguments become a request, the request's own method is read back rather than restated, and the transport's
answer becomes a result. The method is taken from the request so that a request kind added later cannot be sent
with the wrong verb; a method the port cannot express is classified through the same failure mapping rather than
with `?`, because it is a crate defect and the mapping already states what a refusal before writing means.

**13. The adapter checks the request deadline before it sends, and refuses rather than fails.**

A call already past its deadline must not start a network write. `RefusedBeforeReaching` is the honest reading —
nothing was sent, so nothing happened — and `AmbiguousAfterReaching` would send a reader investigating an effect
that never existed. `jarvis-tools`' filesystem adapter makes the same choice before its first read, and
`ToolExecutionRequest::is_past_deadline` exists to be called at the point the adapter chooses.

**14. The pure `run` method takes `now` as a parameter; the trait `execute` reads the clock.**

Split so the deterministic layer can assert an exact `reported_at` in a test while the trait implementation
supplies the real instant. It also keeps one implementation of the logic: `execute` checks the deadline and then
delegates, so the argument mapping is asserted once.

**15. The deadline guard was falsified twice with compiling mutants.**

Deleting the check → detected. Turning the deadline refusal into `AmbiguousAfterReaching` → detected by a single
test, which is what pins the *variant* rather than merely the existence of a refusal.

## Consequences

- **The ambiguity is preserved end to end.** A `Timeout` reaches a caller as `AmbiguousAfterReaching` and a
  `Connect` failure as `RefusedBeforeReaching`, so the pipeline's retry decision has the information it needs
  rather than a guess.
- **A new failure variant cannot default into a silent classification.** Both `may_have_reached_the_provider`
  and the response classifier are wildcard-free `match`es.
- **Every prior slice is now reachable from one entry point.** The contract, the auth flow, the client's
  classification, the definitions, the request builder, and the credential boundary all feed `GoogleReadTool`
  — which is the `P1`/`P3-006d` pattern: a capability a caller can use, not one an operator can reach. It is also
  a **`ToolExecutor`**, so the pipeline can dispatch to it, and a test drives the trait with a request the real
  policy engine authorized rather than a hand-built fixture.
- **`async-trait` became a dependency of this crate.** The one new edge, and it exists so the port can be
  object-safe `async` rather than because the rules here need it.
- **Four guards were falsified with compiling mutants**: making `Timeout` certain (4 tests detected),
  making `Body` certain (3 tests detected), ignoring the status in the response classifier (2 tests
  detected), and deleting/altering the deadline guard (each detected by 1 test). Each mutant compiled, so a
  `FAIL` verdict is a real detection rather than a build failure.

## Limits

- **There is no transport implementation.** The port has a test double and nothing else, so **no request has
  been sent to Google and no response has been parsed from Google**. Every fixture in the tests is constructed
  from `docs/research/integrations/google.md`, so they prove this layer implements the *record* and not that the
  record matches the provider.
- **No `reqwest` transport exists, so the port's own requirements are unenforced.** The module doc states that an
  implementation must not follow a redirect, must not retry, and must not read a proxy from the environment —
  and nothing yet checks any of those, because the only implementations are test doubles. They become testable
  only when a real transport is written, which is why `jarvis-models`' client states the same three rules with
  `redirect(Policy::none())` and `no_proxy()` visible in the builder.
- **No credential is minted or refreshed.** `GoogleReadTool` takes an `AccessToken` it did not obtain, so there is
  no token source, no refresh exchange, and no expiry handling — a stale token becomes a provider refusal, which
  is honest but means a long-lived adapter needs a token source before it is usable.
- **The deadline is read from the system clock, so it is not testable at the trait boundary.** `execute` calls
  `UtcTimestamp::now(&SystemClock)`, which means a test can only assert a *lapsed* deadline and not a
  just-in-time one; the deterministic layer takes `now` as a parameter for exactly that reason.
- **Nothing consumes the `OperationError` type outside this crate, and nothing constructs the adapter in a
  binary.** The mapping onto `AdapterError` is asserted and the trait is implemented, and no registry, executor,
  or route offers `google.gmail_messages_list`, so the adapter is reachable from a test and not from a run.
