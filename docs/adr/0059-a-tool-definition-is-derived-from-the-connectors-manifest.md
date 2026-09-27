# ADR-0059: A tool definition is derived from the connector's manifest, never written beside it

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google connector's tool definitions).
- **Relates to:** `ADR-0023` (`P3-006d`, where the same rule was established for a native adapter).

## Context

A connector declares its operations in a manifest: an id, a description, effects, a risk, the JARVIS scopes a
caller must hold, and a provider-idempotency claim. A model-facing tool needs a `ToolDefinition`: an
identifier, a version, a title and description, an input and output schema, effects, a risk, scopes, an
approval policy, a timeout, a retry policy, an idempotency declaration, a source, availability, and a
sensitivity.

Two of those overlap. The question this slice had to answer is what happens to the overlap, and there were two
answers available:

- **Write a second list.** Simpler to read, and wrong for a reason `P3-006d` already recorded: "policy decides
  about a `ToolDefinition`, so a copy would be policy deciding about a tool **other** than the one being run."
  A hand-written list of tools beside the manifest is two answers to one question, and they diverge the first
  time either is edited — with the divergence appearing as a policy decision about a tool whose declared risk
  no longer matches its effects.
- **Derive one from the other.** More code in one place, and the manifest becomes the single source of every
  security-relevant fact about an operation.

There is also a constraint that decides part of the answer: `ToolSource` is **derived** from an identifier's
namespace rather than declared, and a definition whose declared source disagrees with its identifier is
refused. So a tool's identifier has to be a real JARVIS identifier, which means the connector's namespace must
appear in it.

## Decision

**1. `definitions()` reads the manifest and produces one `ToolDefinition` per declared operation.**

`crates/jarvis-connectors/src/google/definitions.rs`. Every effect, risk, scope and idempotency value comes
from the `ValidatedOperation` it belongs to. Only what the manifest has **no field for** is stated here: the
schemas, the title, the timeout, and the retry policy.

**2. The identifier is `google.<operation id>`, and the prefix is load-bearing.**

`ToolSource::from_namespace` classifies by the **leading segment**, so the `google` prefix is what makes these
`Connector` tools rather than `Native` ones. That is why a tool's source cannot be declared: a connector tool
that could declare itself `Native` would be claiming JARVIS wrote the adapter for a third party's API.

**3. The manifest's `ProviderIdempotency` maps onto `jarvis-tools`' `Idempotency` explicitly and totally.**

Two vocabularies for one question, so a default would answer it silently. The mapping is a `match` on every
variant, and the distinction that matters is between the two *safe* ones: `Declared` means the provider makes
a repeat a no-op, so JARVIS needs no key (`Idempotency::ProviderKey` in `jarvis-tools`' terms is about *who
supplies* the key, and a `Declared` provider needs nobody to); `ProviderKey` means the caller must supply one
(`Idempotency::Required`). Swapping them would either demand a key the provider ignores or omit one it
requires. `Unknown` and `NotIdempotent` both map to `Unsupported`, which refuses repeats — they stay
distinguishable in the manifest for a reader, and collapsing them here is safe because neither claims a repeat
is safe.

**4. The retry policy consults the manifest as well as the effect.**

`RetryPolicy::blind` refuses a retry only for a **mutating** effect. A read-only operation passes that check
whatever its idempotency says, so the check alone would let this module retry everything. `retry_declaration`
therefore also consults `ProviderIdempotency::permits_automatic_retry`, because a blind retry is only free
when repeating the call is free. An operation whose provider behaviour is `Unknown` gets **no** automatic
retry however harmless its effect looks.

**5. The approval policy is derived from the validated risk, not declared.**

`ApprovalPolicy::for_risk` is the guidance table's own mapping, and the manifest already refuses an operation
whose risk is below its effects' floor. So deriving the policy cannot under-approve — and a future write
operation cannot be added with `Auto` by accident, because it would have to raise its risk first, which
raises the policy.

**6. A manifest operation with no schema in this module is refused, not given a permissive one.**

`GoogleToolError::NoSchema` is what keeps the two lists in step. Inventing a permissive schema from the
identifier would advertise a tool whose arguments are unconstrained, and the defect would surface as a model
calling a tool with arguments nobody validated rather than as a build failure.

**7. The input and output classifications differ, and the output is the higher one.**

A message id is `Internal`; the message is `Confidential`. A single field would have to be the maximum, which
would over-restrict the input and hide what the tool actually consumes — and `ToolSensitivity` carries two
values for exactly this reason.

## Consequences

- **A definition cannot disagree with the manifest**, which is the property `P3-006d` established and this
  slice extends to a connector. Changing an operation's effects in the manifest changes the tool, and there is
  no second place to forget.
- **Two Google facts are now constants with tests**: `TOOL_TIMEOUT_SECONDS` (30, a JARVIS choice) and
  `TOOL_BACKOFF_CEILING_SECONDS` (32, Google's lower published `maximum_backoff`). A test asserts the
  relationship and states the real consequence rather than pretending it is fine: two attempts at 32 s cannot
  both complete inside a 30 s deadline, so **only the first retry is reachable**. That is recorded as a limit,
  not smoothed over.
- **A risk-ceiling check was written and then removed as unreachable.** `ValidatedOperation` has private
  fields and is built only by `ConnectorManifest::new`, which already refuses a risk above the platform
  ceiling. So the check could never fire, and an unreachable refusal reads as protection while enforcing
  nothing — the defect `P5-001` and `P5-003` each recorded from a different direction. Its test was replaced
  by one that asserts the *reachable* path enforces the ceiling, so the removal is checked rather than
  claimed.
- **Nothing can call these tools.** No executor, no transport, no token source — the definitions are a
  contract, and the manifest's `CompatibilityVerdict::Unverified` still stands.

## Limits

- **The definitions are not registered anywhere.** No `ToolRegistry` holds them, so `google.gmail_messages_read`
  names a tool a model cannot discover or call. `P3-006d`'s pipeline is the consumer, and wiring a connector
  into it is not this slice.
- **No executor implements `ToolExecutor` for this connector**, so there is nothing for the pipeline to
  dispatch to even if the definitions were registered.
- **The schemas are this connector's construction, not Google's.** Each is a deliberately narrow subset of
  what the API accepts — `format=raw` is absent, `max_results` is bounded by the documented cap — and none has
  been validated against a real response, because no request has been sent.
- **`output_schema` describes a normalised shape, not Gmail's message resource.** That is deliberate (a tool's
  output is JARVIS's, not the provider's), and it means the schema cannot be checked against a provider
  document. No fixture exists.
- **The retry reachability problem is real and unfixed.** A 30-second deadline with a 32-second backoff ceiling
  means the declared second attempt cannot complete. Fixing it means either a longer deadline or a smaller
  ceiling, and neither can be decided without a measured provider latency — which needs a live call.
- **`GoogleToolError::NoSchema` cannot be reached from a test**, because the operation it would be built from
  cannot be constructed: `ValidatedOperation`'s fields are private. The three lookup tables are tested
  directly instead, and the accepted direction is covered through a real manifest. The refusal path is
  therefore verified by construction rather than by exercise, and that is stated rather than implied.
