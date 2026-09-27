# ADR-0057: A declaration that cannot be made honestly is a missing variant, not a default value

- **Status:** Accepted
- **Date:** 2026-09-27
- **Supersedes:** nothing. **Amends:** `ADR-0054`'s `WebhookSupport::Polling`.
- **Slice:** `P5-005` (the Google connector's manifest).

## Context

`P5-005`'s first job is to write the Google connector's manifest, and a manifest is a document whose whole
purpose is that an operator can read it before granting a connector access. `AGENTS.md` is explicit that "a
recorded claim is read downstream as verified evidence", and `P5-004`'s research record is the only authority
for every value in it.

Writing that manifest produced **two places where the existing types could only be satisfied by inventing a
fact**. Both are recorded here because the fix is a shape, not a value, and the shape will recur with every
provider.

### The first: `WebhookSupport::Polling` demanded a number nobody had

`ADR-0054` declared:

```rust
Polling {
    /// The provider's documented minimum polling interval, in seconds.
    minimum_interval_seconds: u32,
},
```

The field's doc says **documented**. Google documents no minimum polling interval for Gmail. Its push guide
recommends falling back to `history.list` "after a period with no notifications for a user" and states no
floor at all. The research record's own "Limits" section records that neither of Google's push mechanisms fits
`WebhookSupport::Push`, so `Polling` is the only truthful declaration available — and the type gave exactly two
ways to make it:

- `minimum_interval_seconds: 60`, which **asserts that Google documents a 60-second floor**. Nothing does. A
  reviewer reading the manifest would take it as a provider fact, and a scheduler planning against it would be
  planning against a number this crate's author chose.
- Declaring `WebhookSupport::Unsupported`, whose own doc says "the provider offers neither". That is false:
  Google offers two push mechanisms and a documented polling fallback.

A `u32` with a documented meaning, at a site where nothing is documented, is a field that converts "we did not
establish this" into a specific false claim. That is the same defect `P5-001` found in `is_keyed_mac`'s doc and
`P5-004` found in `SignatureAlgorithm::None`'s, reached from a new direction: not a claim that a check exists,
but a type that forbids saying the true thing.

### The second: an `AuthFlow` could not be constructed without inventing a redirect URI

`AuthFlow::new` refuses an OAuth method with no `redirect_uri`, with the reason quoted in `ADR-0054`'s research
record: RFC 9700 §2.1 requires **exact** redirect matching. That requirement is right. What it exposed is that
`P5-004`'s record does **not** establish which loopback redirect form Google's client registration expects. The
record has RFC 8252 §7.3's generic rules — `http://127.0.0.1:{port}`, "the server MUST allow any port",
`localhost` NOT RECOMMENDED — but nothing about Google's registered redirect, and Google compares the value
byte-for-byte.

So constructing the flow for this manifest would have meant writing a string that a provider matches
exactly, on a guess. The manifest now exposes `authorization_endpoint()` as a bare `https` constant and **does
not construct a flow at all**, and the missing redirect form is named as the client slice's first task.

## Decision

**1. A declaration that cannot be made honestly is a missing variant, not a default value.**

Where a type asks for a provider fact, the type must be able to represent "not established" as a **distinct
variant carrying no value**, so that an author who did not establish it cannot produce a value that reads as
though they had. This is the rule the crate already applies to `Classification`, `RateLimitEvidence`,
`ProviderIdempotency`, `ResidencyVerification`, `ProbeOutcome` and `RetryClass`; the failure here was one field
that predated it.

**2. `Polling` now carries a `PollingInterval` rather than a `u32`.**

```rust
Polling {
    interval: PollingInterval,
},

pub enum PollingInterval {
    Documented(u32),
    Observed(u32),
    Unknown,
}
```

The variants **carry** the number rather than sitting beside an `Option`, and that is the load-bearing part: a
separate evidence field would make `Some(3600)` beside `Unknown` representable — a figure and a disclaimer that
contradict each other with nothing choosing between them. Attaching the number to the variant that has one
makes the contradiction unrepresentable.

**3. `Observed` is a distinct variant from `Documented`, and `is_documented()` is narrower than
`seconds().is_some()`.**

A floor observed from provider responses can move with a provider's backend. A caller that cannot tell the two
apart will treat one as the other, and the direction that hurts is a scheduler planning against an observed
floor as if it were a published limit. This mirrors `RateLimitEvidence`'s existing reasoning, which is why the
two use the same shapes and the same words.

**4. `Unknown` is not a refusal.**

It exists because a provider may genuinely have no stated floor. Refusing it would leave a connector with only
the two dishonest options above; `PollingInterval::Unknown` says "I poll, and I will not claim a floor the
provider never set".

**5. A declaration that needs a provider fact this research does not establish is deferred, not guessed.**

`GoogleConnector::authorization_endpoint()` returned a bare constant and no `AuthFlow` was constructed. The
refusal to construct one was the decision; the redirect form belonged to the slice that could look it up.

**Resolved within the same slice, and the resolution is recorded rather than the decision rewritten.** Two
authoritative sources supplied the missing facts: `https://accounts.google.com/.well-known/openid-configuration`,
which is **machine-readable** — the server's own published configuration rather than a page's example — and the
OAuth 2.0 for native apps guide. The flow is now constructed from `authorization_endpoint`, and the registered
redirect is the **portless** loopback form that `LoopbackRedirect::registered` produces and
`matches_except_port` compares.

What did **not** get resolved, and is now Unresolved Question 7 in the research record, is narrower and
different in kind: Google's page shows the *exchange request* using a ported URI and requires an exact match
against an authorized URI, but does not state which string the Cloud Console accepts as the **registered** value
for a Desktop-app client. That is one string a human types, not a protocol requirement — and the test asserts
the comparison that joins the two forms works **while explicitly not claiming** the console accepts the value.

## Consequences

- **The Google manifest can be written honestly.** It declares `Polling { interval: Unknown }`, which is true:
  Google has push, Google documents a polling fallback, and Google documents no minimum interval.
- **`ADR-0054`'s `Polling` shape is amended, and the two existing call sites updated** (`manifest_tests.rs`,
  `readiness_tests.rs`). `PollingInterval` is exported from the crate root, because a connector author needs it
  to write a manifest.
- **The scaffold's `_comment` was updated**, since it documented the JSON form an author copies. A generated
  skeleton that showed the old field would produce manifests that fail to parse — and the comment now says
  explicitly to use `"unknown"` rather than inventing an interval, because the comment is documentation an
  author reads at the moment they are about to choose a number.
- **The Google connector is declared `CompatibilityVerdict::Unverified`**, whose `is_installable()` is false.
  That is deliberate and is the honest state: nothing has been run against Google. It also means the manifest is
  a contract under construction rather than something an operator can install, which is better stated than
  discovered.
- **`jarvis_connectors::google` is a public module**, not a set of re-exports into the crate root, because
  `CONNECTOR_ID` becomes ambiguous the moment a second provider arrives.

## Limits

- **Nothing in the Google manifest has been exercised against Google.** No credential exists, no request has
  been sent, no wire fixture has been recorded. Every value is transcribed from a dated source, and the tests
  assert internal consistency and agreement with the research record — not provider behaviour.
- **The operation ids are declarations, not registered tools.** No `ToolDefinition` is derived from them yet and
  no `ToolRegistry` holds them, so `google.gmail_messages_read` names nothing a model can call.
- **The Gmail rate limit's `burst` is a judgement.** `per_window` and the scope come from Google's documented
  quota table; the burst is set below the per-user ceiling on the reasoning that a burst ignoring the tighter of
  two limits plans against the wrong one. That reasoning is recorded in the code, and it is an interpretation of
  the figures rather than a figure Google publishes.
- **`is_documented()` has no production caller yet.** It exists so a scheduler can branch, and nothing schedules.
  By `P5-001`'s own standard that is a predicate whose enforcement is deferred to its first caller — noted
  rather than left to look like coverage.
- **No `AuthFlow` exists for Google**, so the loopback listener and the redirect form are unbuilt. This is
  recorded as a task rather than a gap in the manifest.
- **Two unresolved questions from `P5-004` are unaffected**: Calendar scopes' sensitivity categories, and
  whether a self-hosted single-user JARVIS qualifies for Google's internal-app exemption.
