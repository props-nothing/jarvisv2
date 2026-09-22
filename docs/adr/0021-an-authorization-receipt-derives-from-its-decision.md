# ADR-0021: An authorization receipt is derived from its decision, and cannot be built from a refusal

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-001` through `P3-006` each built one piece of the tool pipeline: a contract, a registry, a policy
engine, a durable approval, a call lifecycle, and one adapter. **Nothing composed them.** No production
code built an `AuthorizationReceipt`; the only constructions in the workspace were six test fixtures.
An adapter had never been handed a receipt that came from a real policy decision.

Closing that gap to write the first seam test found that it was a **security** gap rather than only an
integration one.

`AuthorizationReceiptParts` took the risk as a **value**:

```rust
pub struct AuthorizationReceiptParts {
    pub intent_hash: String,   // any string
    pub risk_level: Risk,      // stated by the caller
    // ...
}
```

`P3-003`'s `PolicyDecision::effective_risk()` was the authority on how much scrutiny a call needed —
`docs/architecture/tools-and-connectors.md` requires that "risk ≥ the effect floor" and that context can
raise it (`External`, `Bulk`, `Sensitive`, `Production` escalations). But **nothing held the decision and
the receipt at once**, so a composer could do this:

```text
evaluate(...)                       -> effective_risk = High
AuthorizationReceiptParts {
    risk_level: Risk::Minimal,      -> accepted; nothing compares the two
    ...
}
```

The receipt is what an adapter treats as permission. So the declaration an adapter acted on could
understate the risk the decision was actually taken at, and no code anywhere would refuse it. The
decision record and the authority would disagree, and the two would be reconciled by nobody — which is
exactly the class of defect this project keeps finding: **two values that must agree, with nothing
holding both.**

The digest had the same shape of problem. `intent_hash` was a `String`, and its contract is that it
equals the digest an approval binds to (`jarvis_core::CanonicalIntentHash`, `P3-004`). A `String` field
cannot express "this is that digest", so a receipt could name an intent that was not the one about to
run.

Two documents make the direction of the fix non-optional.

`docs/architecture/security.md`: "Authorization evaluates actor, client, workspace, capability, connector
account, resource, channel, effect, risk, and policy version. **Deny overrides allow. Missing or stale
evidence fails closed.**"

`AGENTS.md`: "The model may request an effect; deterministic Rust policy decides whether it may happen."

## Decision

**1. `AuthorizationReceiptParts` carries the `PolicyDecision`, and the receipt's risk is derived from
it.** A `risk_level` field no longer exists. `AuthorizationReceipt::new` reads
`decision.effective_risk().level()`. The two values cannot disagree because only one of them exists as
an input. This is the same move as `P3-001`'s `ToolSource` being derived from the identifier and
`P3-002`'s availability taking the restrictive of two sources: **derive the value that must agree, or
the agreement is a check somebody can skip.**

**2. A receipt cannot be built from a decision that did not authorize.** A `Deny` returns
`ReceiptError::DecisionNotAuthorizing`, and so does a `RequireApproval` with no approval cited. The
receipt is an authority; building one from a refusal would turn a refusal into permission. Enforced in
the constructor rather than at call sites, because a call site can forget and there will be several.

**3. The digest must be the digest of the tool, version, and arguments the receipt covers.** The parts
carry the `arguments`, and `new` recomputes the canonical digest with `CanonicalIntentHash::compute` and
refuses a mismatch with `ReceiptError::IntentMismatch`. The digest still arrives **pre-computed** rather
than being computed here, because `jarvis-tools` does not depend on `sha2` and computing it would mean a
second implementation of the canonical form — and the canonical form's separator (`\u{1f}`) is precisely
what stops one intent's digest matching a different tool's. One implementation, checked at the boundary.

**4. The receipt stores the digest's own canonical hex.** `intent_hash.to_hex()` rather than a
caller-supplied string, so a receipt's `intent_hash` and an approval's `intent()` are the same string by
construction rather than by convention.

**5. A fixture may not fabricate a decision.** `PolicyDecision`'s fields are private and it is built only
by `evaluate`, and the new test helpers go through `evaluate` rather than around it. A test that could
construct a decision directly could fabricate authority, which would make the seam test prove nothing
about the seam.

## Consequences

- The seam is now **tested rather than asserted**: `a_receipt_cannot_disagree_with_the_decision_that_authorized_it`
  pins all three properties, with controls proving each refusal is about the intended cause (a matching
  digest *is* accepted; a held decision *with* an approval *does* produce a receipt).
- A registry mistake has a vocabulary: an adapter handed a request for a tool it does not implement
  returns `AdapterError::NotImplemented`, which is what makes "a tool was advertised that no adapter can
  run" visible. The receipt for that case must still be validly built, which is why the fixture
  evaluates against a known tool and names an unknown one.
- `ReceiptError` gained two variants, both actionable and specific: `DecisionNotAuthorizing` and
  `IntentMismatch`. Neither is a generic "invalid request", because the two failures have different
  fixes — an authorization-flow bug versus a hashing bug.
- The change was **forced by composition, not by review**. Six slices had been written and reviewed
  without anyone noticing, because each slice was self-consistent: `P3-003` had no receipt to disagree
  with, and `P3-005` had no decision to be checked against. The defect existed only in the space between
  two correct modules, which is the space no unit test covers.

## Honest limits at the time of this decision

- **This is a library fix, not a pipeline.** There is still no composition root: nothing in
  `apps/jarvisd` reads a definition from a registry, calls `evaluate`, requests or answers an approval,
  admits a tool call, or calls an adapter. The seam is now *correct and tested*; it is not *driven*. The
  remaining composition work — registry lookup, approval round-trip, admission, outcome recording — is
  `P3-012`'s, and this fix removes the authorization hazard from it rather than doing it.
- `AuthorizationReceiptParts`' `arguments` field means a receipt construction clones the argument
  object. Accepted: it is needed to verify the digest, the object is already bounded by the tool's input
  schema, and the alternative is trusting a caller-supplied hash.
- A receipt still cannot be **revalidated** by a third party. It records what was decided; re-checking it
  against a current definition and policy version at execution time is the resume path `P3-012` owns, and
  `is_valid_at` covers only the approval's lapse.
- The `policy_version` is still a caller-supplied string with no registry of versions to check it
  against, so it is recorded but not verifiable.
- The digest covers tool, version, and arguments — **not** the actor, workspace, or channel. An approval
  binds to the intent, and the actor's authority is a separate evaluation; whether the digest should also
  cover the actor is a question `P3-012` should answer, because today two actors with the same tool and
  arguments produce the same digest.

## Alternatives considered

- **Keep `risk_level` and add a runtime check comparing it to the decision.** Rejected: it is a check a
  caller can skip, which is the defect rather than the fix. Deriving the field makes the disagreement
  unrepresentable.
- **Take `effective_risk: Risk` instead of the whole `PolicyDecision`.** Rejected as weaker: it prevents
  the risk from being *wrong* but still allows a receipt from a `Deny`, because a denial also has an
  effective risk. The whole decision is what makes rule 2 expressible.
- **Compute the digest inside this crate.** Rejected: it would mean a second implementation of the
  canonical form, and the `\u{1f}` separator between tool and version is a subtle detail a second
  implementation gets wrong — `("a","bc")` and `("ab","c")` would then hash identically.
- **Keep `intent_hash: String` and validate its shape (64 lowercase hex).** Rejected: shape validation
  proves the string *looks* like a digest, not that it is *this* intent's digest. The failure mode is an
  authorization carried to a different action, which looks perfectly well-formed.
- **Make the receipt's construction private and expose a `permission_for(decision)` entry point.**
  Rejected as an equivalent that hides the same property behind a name; taking the decision as an input
  already makes the association structural, and the constructor's documentation can say why.
- **Let a `Deny` produce a receipt marked denied, for the audit record.** Rejected: the audit record is
  the call row and the decision, not the receipt. A receipt that can describe a refusal invites an
  adapter to receive one, and "the adapter was handed a denial" is exactly the confused-deputy shape
  this refuses.
- **Require an approval for every receipt.** Rejected: a risk-0 read allowed by policy needs none, and
  inventing one would make `Auto` approval meaningless.

## Conditions that would justify revisiting

- `P3-012`'s composition reveals a caller that legitimately holds a decision and a receipt that are not
  about the same call, which would mean the association is too strict.
- The resume path needs a receipt that outlives its decision's `policy_version`, which would require the
  version to become verifiable rather than recorded.
- A second adapter area (MCP, `P3-008`) needs to build receipts for tools whose definitions arrive at
  runtime rather than from a registry, which would test whether taking a `PolicyDecision` is compatible
  with a dynamically discovered tool.
- The digest's scope is revisited to include the actor or workspace, which changes what an approval binds
  to and therefore needs its own ADR.
