# ADR-0054: A connector is described before it is trusted, and its declarations are checked against each other

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P5-001` requires the connector contracts: manifest, account, auth flow, health, sync cursor, webhook,
rate-limit, scope, and diagnostics. `docs/architecture/tools-and-connectors.md` requires the manifest to be
**"parseable without loading provider code"** and lists what it must contain.

That sentence reads like a packaging constraint and is actually a **trust** constraint. The manifest is the
only artifact an operator reads before granting a connector access to their mail or calendar, and the provider
code it describes is precisely what has not run yet. So the manifest is a security document, and the question
this slice had to answer was not "which fields does it need" but **"which of its own claims can be checked
against each other, and which must be refused when they disagree"**.

The crate is `jarvis-connectors`, an adapter by `docs/architecture/repository-layout.md`'s dependency rules: it
depends on `jarvis-core` and `jarvis-tools` and nothing else in the workspace, and no adapter depends on it.

Three findings shaped the design and were not obvious up front.

1. **The dangerous direction of every declaration is the one that under-reports.** `P3-008b` established this
   for risk: risk is a *floor raised by effects*, so the ladder runs opposite to a vendor's incentive. The same
   asymmetry recurs at three separate places in this crate — a declared classification, a declared
   compatibility claim, and a declared retry class — and in each case the cheap mistake (under-report) is the
   one that grants unearned latitude while the expensive mistake (over-report) only inconveniences.
2. **A "documentation link" is not a property.** `docs/development/external-research.md` names an *ordered*
   source list: repository-local `llms.txt`, official documentation, official specification, official SDK
   source, then release notes. A manifest with a marketing homepage has a link and has discharged nothing.
3. **Writing the guards produced two real defects in this crate that the first draft's tests did not catch.**
   A whitespace-only provider account identifier was accepted, and a *future* health observation was reported
   as fresh. Both are recorded in the decisions below, because in both cases the implementation and its own
   doc comment disagreed.

## Decision

### 1. Effects are declared per operation in the manifest, and a classification below them is refused

Every operation declares its effect set, and `EffectSet::new` refuses an empty one — "this does nothing" is not
a representable statement about a provider call.

The check that earns its place is the **cross-check**: a connector whose operations reach outward may not
declare `Classification::Public` or `Internal`. `Classification::may_reach_a_remote_model` is what makes this
concrete rather than stylistic — those are exactly the levels permitted to reach a model, so the combination
mislabels content that leaves the machine.

The rule is a **floor, not an equality**. A mail connector legitimately declares `Confidential` regardless of
its effects, because what it *handles* is confidential. Requiring equality would force every outward connector
to the top of the ladder, which makes the classification meaningless. The test asserts all three directions:
the two low levels refused, every level at or above `Confidential` accepted, and a read-only connector allowed
to declare `Internal`.

### 2. A secret is a *field name*, never a value

`SecretField` names what a deployment must supply and says what kind of value it is. There is no `String` field
anywhere in the crate that holds token material, and that absence is the mechanical form of `security.md`'s
rule rather than a convention someone has to remember.

The name is bounded and constrained — lowercase with underscores, unique, 1–64 characters, with a stated
purpose — because it becomes an environment variable name and a rendered configuration key. A field name that
could not be an environment variable would fail at deployment, which is the worst place to learn it.

### 3. Documentation links must discharge the research requirement

`LinkKind`'s variants **are** `external-research.md`'s ordered source list, not a generic taxonomy:
`LlmsTxt`, `Documentation`, `Specification`, `Sdk`, plus `ApiDescription`, `Changelog`, `Terms`, `Support`.

`DocumentationLinks::new` refuses a set in which **no** link satisfies `satisfies_research_requirement()`. So
a manifest with only a homepage is incomplete rather than merely terse, and the refusal names the requirement.
The remaining bounds — at least one link, at most sixteen, distinct kinds, `https://` only, a stated purpose —
exist because this type is the record that a later reader uses to re-verify the API after it changed.

The link set also carries a `ResearchRecord`: the repository-relative path of the dated research document and
its ISO date. The path is refused if absolute, if it contains `..`, or if it contains a `:`, because the value
is a repository-relative reference and a `C:\` or `../` prefix means it points somewhere the manifest does not
control.

### 4. The risk floor is checked here, at the first place an operation's risk is stated

An operation may not declare a risk below its effects' floor, and may not exceed
`jarvis_tools::MAX_RISK_LEVEL`. This is the first point in the pipeline where risk is expressed at all, so it
is the first point where the floor can be enforced.

**What is deliberately *not* checked here, and why that is not an omission:** the `P3-001` rule that a blind
retry of an ambiguous effect is a second effect. A non-idempotent outward operation is legitimate and common —
sending a mail is exactly that — so refusing one would refuse most connectors. What must not happen is a
*retry policy* declared alongside a provider that duplicates the effect, and **this type carries no retry
policy to disagree with**. That pairing becomes expressible in `P5-009`, where an operation becomes a
`ToolDefinition` and gains a `RetryDeclaration`. There is nothing to check yet rather than a check that was
forgotten, and the boundary is stated in the module so a later reader does not "fix" it by adding a refusal
that would break every write connector.

### 5. A cursor's *kind* names what may be concluded from it, and `Start` is a variant rather than an absence

`SyncCursorKind` is `OpaqueToken | MonotonicMarker | Etag | Offset | Start`, and each carries two derivable
questions: `can_be_detected_as_stale()` and `needs_full_resync_when_lost()`. The kind is not decoration — an
`Etag` and an `Offset` fail in different ways and a caller that only stored a `String` would have to guess.

`Start` is a **variant with no token**, not "no cursor yet", and `SyncCursor::new` refuses a `Start` carrying a
token as well as any other kind carrying none. The reason is in the refusal: a `Start` with a token makes
"never synced" indistinguishable from "synced to here", which is the single distinction this type exists to
keep. A full resync is then a decision a caller makes via `requires_full_resync()` rather than a default it
falls into.

A cursor is bound to the account **and the connector version** that produced it (`applies_to`), because a
cursor from version 1.0.0 applied after an upgrade to 2.0.0 is a silent misread. `SyncWindow` bounds a run at
both ends — items and seconds — with ceilings, and the ceilings refuse rather than clamp.

### 6. Health carries the probe that produced it, and staleness takes a *supplied* instant

`ConnectorHealth` is `Connected | Degraded | NeedsReauth | Disconnected | Unknown`, and every variant carries a
`HealthSignal` naming the `HealthProbe` that observed it. A state without its probe is an assertion with no
evidence: "connected" measured by an identity call and "connected" measured by a subscription are different
claims.

`is_fresh_at(now, freshness_seconds)` takes the instant as a parameter rather than consulting a clock, because
`jarvis_core::Clock` exists so time is injectable and a value that asked the system clock about its own age
could not be checked against a supplied instant. Comparison is on `unix_nanos`, not on the RFC 3339 text:
`jarvis_core::UtcTimestamp`'s rendered form omits a zero fraction, so two instants in one second do not sort
lexicographically (`P3-004`'s recorded trap).

**Two defects this slice found in itself, both from writing the tests rather than the code:**

- **A whitespace-only provider account identifier was accepted.** `VerifiedAccount::new` checked
  `provider_account_id.is_empty()` rather than the trimmed value, so `"  "` passed a check whose whole purpose
  is to refuse an unusable identifier. Fixed to `trim().is_empty()`.
- **A future observation was reported as fresh.** The doc comment already said a future observation is not
  fresh, but the implementation only rejected `checked_sub`'s `None` (overflow) case, so a `Some(negative)`
  fell through to the bound comparison. The failure direction is the dangerous one: treating a future
  observation as fresh lets a record whose clock moved backwards look current. Fixed with an explicit
  `if elapsed < 0 { return false; }`.

`permits_calls_at(now, freshness_seconds)` exists so that combining the two is a method rather than a call-site
obligation: calling `permits_calls()` on an unfresh state is the defect the pair prevents.

### 7. `Unknown` never permits a call, and `RetryClass::Unknown` never retries whatever the idempotency says

Two rules with the same shape, and the shape is the point: **the unanswered question refuses.**

`ConnectorHealth::Unknown` does not permit calls, and `permits_calls_at` cannot be talked out of that by
freshness. `RetryClass::Unknown` returns `false` from `permits_automatic_retry` **in every column**, including
when the provider's idempotency guarantees the retry is safe. That is deliberate and is the opposite of the
other classes: `Transient`, `Throttled` and `ProviderFault` delegate to the idempotency claim, but `Unknown`
refuses because *the question is unanswered* rather than because the provider said no. A retry under an
unclassified failure is a second effect with no basis.

`RetryDecision` deliberately is not `Copy` — it carries an optional `ProviderRequestId` — and the class travels
with its `RetryGuidance` so a delay cannot be read without also reading the class that justifies it. A retry
delay longer than a caller may hold is refused rather than clamped, because clamping silently changes a
provider's instruction into a different one.

### 8. A webhook body is raw bytes, and an ambiguous security header is refused rather than picked

`WebhookDelivery` exposes `body: &[u8]` and `headers: &[(&str, &[u8])]`. The bytes are never pre-parsed,
because signature verification is over the **raw** bytes and a delivery that has been through a JSON parser is
no longer verifiable.

`single_header` returns `None` for **both** zero values and more than one. This is the single most important
line in the module: a repeated security header is either an accident or an attack, and picking the first would
turn "two signatures disagree" into "one signature was valid". The refusal is named —
`WebhookRejection::Replayed` and friends, with `indicates_an_authenticity_failure()` distinguishing a forged
delivery from a merely duplicate one, and only `Replayed` is acknowledged to the provider (so a genuine retry
stops, while a forged delivery gets no signal).

`SignatureScheme` requires a lowercase token header, and `WebhookBinding` requires an absolute path with no
query, no fragment and no `..`, plus at least one of an account header or a body field — because a webhook that
cannot say which account it arrived for cannot be routed, and one with neither is a delivery nobody can
attribute.

### 9. The diagnostics field set is closed, and `is_loggable()` is true for all of it

`DiagnosticField` has twenty variants and `is_loggable()` returns `true` for every one. The constancy is the
point: the type is a **closed set chosen so that this can be true**, rather than a set with a `may_log`
exception list. A field that needed an exception would mean a value in this type was not safe to log, which is
an invariant worth keeping absolute.

`may_reach_a_model()` is false only for `ProviderRequestId`, and **not because it is a secret** — it is an
identifier of a request the provider already logged. It is withheld because `security.md` minimizes what
reaches a model rather than deciding case by case at each call site: putting a provider's request identifier in
a prompt invites a model to reason about it. Everything else describes this platform's own state.

A cursor's token is **never** a field, only `CursorKind` and `CursorObservedAt`: a cursor is provider-issued
text that can address another account's data, so the age is recorded and the token is not. `MissingScopes` is
the deliberate exception to the "don't name what the account has" rule, because a missing scope is what the
user must act on and it names what the connector *wants* rather than what the account *has*.

The test carries a hand-written `all_fields()` list and asserts the count is twenty, so adding a variant
without considering its logging and model exposure fails rather than passing silently.

### 10. PKCE is verified against RFC 7636's own numbers, and the flow refuses contradictory shapes

`PkceVerifier` enforces RFC 7636 §4.1's 43–128 characters and the unreserved alphabet, and the S256 challenge
is asserted against **Appendix B's published test vector** rather than against a second implementation of the
same mistake. `PkceVerifier` is hand-written `Debug` printing a length, because a verifier is the one value
that must never appear in a log.

`AuthFlow::new` refuses every contradictory combination rather than normalizing one: an OAuth method without a
PKCE method or a redirect, a non-OAuth method *with* either, an authorization endpoint that is not `https`, and
a redirect that is not loopback. `is_loopback_redirect` matches whole hosts (`127.0.0.1`, `[::1]`, `::1`) over
`http://` only — a substring check would accept `127.0.0.1.evil.example`, which is the classic open-redirect
shape.

`ScopeChange::between` is a set operation, not a comparison, and **loss takes precedence** over gain: a refresh
that gains one scope and loses another is a scope loss, because the loss is what breaks calls.

`SecretValue` and `VerifiedAccount` both have hand-written `Debug` that redact, and `SecretValue::matches` is
constant-time by content. `VerifiedAccount`'s provider account identifier is redacted in `Debug` too, because it
appears in diagnostics and a redacted type is cheaper than a redaction call at every site.

### 11. Every guard is falsified, and the falsification is three runs rather than two

Each of the twelve guards above was mutated and its test re-run: **A** (guard intact) must pass, **B** (guard
neutered) must fail, **A′** (restored) must pass.

A′ is not ceremony. Restoring a file with `Copy-Item` sets its mtime to the *backup's* timestamp, which is older
than the mutant build cargo just produced, so cargo can consider the mutant artifact fresh and run it again.
That exact failure occurred here: a guard whose test genuinely catches it reported "SURVIVED", and the next
command still failed *after* the file was verified byte-correct. Without A′ the verdict would have been recorded
as "not load-bearing" — the opposite of the truth.

The first harness also produced two false "SURVIVED" verdicts because the *mutant* was wrong rather than the
guard: `elapsed < -1000000` is still true for a one-second-future instant, so it removed nothing, and one probe
mutated `needs_full_resync_when_lost` while the test exercised `requires_full_resync` — a different, uncalled
method. Both are recorded because a green falsification run is worth exactly as much as the mutant's precision.

### 12. A `push` declaration whose scheme verifies nothing is refused — added during `P5-004`

`WebhookSupport::Push` carries a `SignatureScheme`, and that scheme may name `SignatureAlgorithm::None`.
`SignatureScheme::authenticates()` exists to detect exactly that value, and its documentation said `None` "is
refused by `WebhookBinding::new`". **It was not.** `WebhookBinding::new` validates the *binding* — the path and
the account header or body field — and never sees the algorithm, which is a sibling field of the same enum
variant. `ConnectorManifest::new` did not validate the webhook at all.

So a manifest could declare `push` with an authenticator of `none` and be accepted, leaving an endpoint that
applies unauthenticated writes from anyone who knows the path: `security.md`'s "webhook spoof" row with its
control removed, declared in a document a reviewer would read as having provided one. `authenticates()` was
called from **tests only**, which is why the gap survived the slice that introduced it.

The fix is `validate_webhook`, called from `ConnectorManifest::new`, plus `ConnectorError::Webhook`. The refusal
is in the manifest rather than in `WebhookBinding::new` because the two types are siblings and neither owns the
other; the manifest is the only object holding both.

**How it was found, and why that matters more than the fix.** The research task `P5-004` required writing down
Google's push mechanisms, which are an OIDC bearer JWT and an echoed channel token — neither of which the
scheme can name. Explaining *why* they could not be named meant reading the type, and reading the type meant
noticing that its `None` doc described a check no code performed. **A documentation claim was the witness**: the
comment asserted protection, the predicate was reachable only from tests, and the constructor named was the
wrong one. The generalizable lesson is that a doc comment saying "refused by X" is a claim about code and must
be read as one; a predicate that only tests call is a predicate that enforces nothing.

## Consequences

- `crates/jarvis-connectors` is a new workspace member, depending only on `jarvis-core` and `jarvis-tools`, so
  it sits below every adapter in the dependency graph and can be consumed by a connector implementation, a
  daemon, or a scaffold generator without an ordering constraint.
- The manifest is checkable **before** provider code is loaded, which is what makes it usable as an install-time
  gate: `ConnectorManifest::new` performs every cross-check in one place, and a manifest that passes is one
  whose declarations do not contradict each other. It is *self-consistency*, not verification — see Limits.
- `jarvis_tools::EffectSet`, `ToolEffect` and `MAX_RISK_LEVEL` are consumed rather than restated, so a
  connector's effect vocabulary and a tool's are one vocabulary. `ToolEffect` is re-exported from this crate so
  a manifest author and a reader do not work in two copies.
- `RateLimit`, `SignatureScheme`, `WebhookBinding` and the signature enums derive `Serialize`/`Deserialize`
  because the manifest contains them. They are the only types in the crate that do, and the derive is what
  forced the manifest to be the single carrier of a rate limit rather than a parallel declaration.
- Nine modules with eighty tests, no external client, and no I/O.

## Limits

- **Nothing consumes this crate yet.** There is no `Connector` trait, no HTTP client, no RNG, no loopback
  listener, no `SecretStore` integration, no persistence, and no daemon wiring. Those are `P5-002` onward. This
  slice defines and constrains the contracts; it connects nothing to anything.
- **`PkceVerifier` is verified, not generated.** There is no RNG dependency, so no code path produces a
  verifier — the crate checks a supplied value against RFC 7636's bound and computes its challenge. Generating
  one needs a CSPRNG and belongs with the flow that uses it (`P5-002`).
- **The manifest checks self-consistency, not truth.** Nothing here can prove that a connector's declared
  effects match what its provider code does. The classification rule catches a manifest that contradicts
  *itself*; a connector that lies consistently is caught by review and by the recorded research, not by a type.
- **No signature verification is implemented.** `SignatureScheme` describes an algorithm, a header and an
  encoding, and `WebhookDelivery` exposes the raw bytes a verifier needs — but no HMAC is computed and no
  public key is parsed. The scheme is a declaration; the verifier is `P5-010`.
- **No pagination is implemented.** `SyncWindow` bounds a run and `SyncCursor` records a position, but no code
  follows a `next` link or interprets a provider's paging shape, which differs per provider and arrives with
  each connector.
- **The rate limit is a declaration.** `RateLimit` records what a provider documents and `RetryGuidance` says
  what to do about a refusal; nothing counts requests, and no budget is shared with a live limiter. Enforcement
  needs a caller (`P5-010`).
- **The ceilings are chosen, not measured.** `MAX_CONNECTOR_OPERATIONS`, `MAX_CONNECTOR_SCOPES`,
  `MAX_RATE_LIMIT_PER_WINDOW` and the rest are bounds that refuse rather than clamp, and no provider was
  measured to derive them. They are deliberately generous, so a measured ceiling should replace them when a
  real connector is written.
- **`DiagnosticField`'s set is a judgement, not a verified minimum.** The twenty fields are the ones this
  slice believes are needed for the failures in `P5-010`, and the count assertion makes growth deliberate
  rather than free. A connector may turn out to need a field that is not here.
