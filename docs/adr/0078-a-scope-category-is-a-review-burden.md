# ADR-0078: A scope's category is a review burden, and an unlisted scope is not a cheap one

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google connector contract).
- **Relates to:** `ADR-0054` (a connector is described before it is trusted), `ADR-0057` (a declaration that
  cannot be made honestly is a missing variant), `ADR-0058` (a provider's decisions belong in the crate with no
  socket), the research record's Unresolved Question 2, `P5-005`.

## Context

Google classifies every OAuth scope as **non-sensitive**, **sensitive**, or **restricted**, and the scopes page
read on 2026-09-27 states what each category costs:

> Non-sensitive: These scopes provide the smallest scope of authorization and only require basic OAuth App
> Verification.
>
> Sensitive: … They require additional OAuth App Verification.
>
> Restricted: These scopes provide wide access to Google user data and require restricted scope OAuth App
> Verification.
>
> **If you store restricted scope data on servers (or transmit), then you must go through a security
> assessment.**

So the category is not a label: it is a **review burden**, and for this connector it is the fact that decides
whether deploying at all needs a Google security assessment.

That fact lived only in prose. The connector declared its scopes as strings:

```rust
scopes: vec![
    SCOPE_OPENID.to_owned(),
    SCOPE_GMAIL_READONLY.to_owned(),
    SCOPE_CALENDAR_READONLY.to_owned(),
],
```

`SCOPE_GMAIL_READONLY`'s own doc comment said "**Restricted** rather than sensitive, which is the fact that
governs what deploying this connector costs" — a correct and useful sentence, and the only place the fact
existed. Nothing could check it, a new scope could be added without anyone deciding its category, and the
research record's own Verification Plan had the item open:

> **Scope-category tests.** A test that a Gmail connector's declared scopes are all *accounted for* against a
> table of Google's categories, so adding a scope forces a decision about the verification burden. The table is
> a fixture with a date, because Google can recategorise. **Not written.**

The item is worth reading closely, because it asks for the right thing: not "assert these three scopes are
restricted" but **"accounted for"** — every declared scope has a decided category. Those are different
requirements, and only the second survives a new scope being added.

## Decision

The category, the burden, and the assessment requirement become types, and the accounting becomes a pure
function over a dated table.

```rust
pub enum ScopeCategory { NonSensitive, Sensitive, Restricted, Unknown }
pub enum VerificationBurden {
    BasicReview, AdditionalReview, AdditionalReviewAndConditionalAssessment, Unestablished,
}
pub enum AssessmentRequirement { NotRequired, IfStoredOrTransmitted, NotEstablished }

pub fn account(declared: &[String], table: &[(&str, ScopeCategory)]) -> ScopeAccounting
```

Five properties are deliberate:

1. **An unlisted scope is not a cheap scope.** [`ScopeCategory::Unknown`] means the published table has no
   entry, and its burden is `Unestablished` — **not** `BasicReview`. Under-reporting a burden is the direction
   that harms: an operator told "basic review" who in fact ships a restricted scope has skipped an assessment
   Google requires, and nothing in the deployment would have said so. The same reasoning `RateLimitEvidence`
   applies to `Documented` versus `Observed`.

2. **The burden fails closed on a partial reading.** [`ScopeAccounting::burden`] returns the heaviest
   established burden only when **every** declared scope is accounted for, and `Unestablished` otherwise —
   because reporting the known scopes' maximum would present a partial answer as a complete one. This is
   load-bearing for the real connector: two of its four scopes (`openid`, `calendar.readonly`) have no
   recorded Gmail category, so the deployment's burden is honestly **unstated** rather than "restricted"
   inferred from the two that are known.

3. **The assessment is a three-valued condition, not a `bool`.** Google's rule is "**if** you store restricted
   scope data on servers (or transmit)". A boolean would make `NotRequired` and `NotEstablished` the same
   value, which is exactly the conflation that lets an unassessed deployment look compliant. `NotRequired` is
   the single case where the published rule positively excludes an assessment.

4. **The match is exact.** A scope string goes into an authorization request, so it is an identifier. A prefix
   or case-insensitive match would let a longer scope inherit a **cheaper** category from a scope that is its
   prefix — the under-reporting direction again.

5. **The table is deliberately partial and dated.** Only the six Gmail scopes this project has a use for are
   listed; every omission becomes `Unknown` in the accounting rather than being guessed. The table's date is
   asserted equal to the research record's `RESEARCH_VERIFIED_ON`, so a recategorisation cannot be recorded in
   one and not the other.

## Consequences

- The research record's open item is **written**, and it reacts to the connector's **real manifest scopes**
  rather than a fixture list, so adding a scope is what makes the test speak.
- `SCOPE_GMAIL_READONLY`'s prose claim is now a checked value, and the test asserting the connector's scopes
  are accounted for fails loudly if a scope is added without a category.
- The three counter-intuitive facts the record calls out are pinned against the constants: `gmail.send` is
  **sensitive** (not restricted), `gmail.labels` is the one generally useful **non-sensitive** scope, and
  `gmail.metadata` is **restricted** despite being the least privileged way to read a mailbox.
- **Two mutants were falsified A-B-A**, both compiling:
  - making `Unknown` report `BasicReview` (detected by `an_unlisted_scope_is_not_a_cheap_scope`);
  - making the table lookup a prefix match (detected by
    `the_scope_match_is_exact_because_a_prefix_would_inherit_the_wrong_category`).
- **A defect in this slice's own first draft was caught by its own test.** `AssessmentRequirement` originally
  carried `is_required_regardless`, whose comment claimed "both answer `false`" while the code returned `true`
  — and, once fixed to match the comment, the predicate was **false for every value**. Google's rule is
  conditional for the only category that requires an assessment, so no category is ever required *regardless*.
  It was replaced by `may_require_an_assessment`, which fails closed and which a caller can branch on. That is
  the same defect class the ADRs `0067`–`0077` found — a declaration no input can exercise — caught here in
  the slice that was writing about it.
- **A limit remains:** the Calendar scope's category and the `openid` scope's category are **not established**,
  because the pages read for them do not state one. This is recorded in the research record (Unresolved
  Question 2) and surfaced by the accounting rather than assumed away. Modelling the **internal-app
  exemption** is deliberately out of scope: it is a property of the consent screen's audience setting, not of
  the scope set, so a field for it could only be set wrongly.

## Alternatives considered

- **Store a category beside each scope constant.** Rejected: the page publishes three *lists*, so the list a
  scope is in is the fact; a category beside each constant would be a second copy that can drift, and a scope
  missing from the table would silently have a category rather than being visible as a gap.
- **Assert the three expected categories directly.** Rejected, and this is what the record's own wording
  already rejected: "assert these are restricted" passes when a fourth scope is added, while "accounted for"
  does not.
- **Treat an unlisted scope as non-sensitive, with a comment explaining why.** Rejected: that is the
  under-reporting direction, and the comment would be the only thing standing between an operator and a missed
  assessment — precisely the arrangement `ADR-0077` had just removed for a different bound.
- **Model the internal-app exemption as a declaration.** Rejected: it would let an author assert the exemption
  for a deployment whose consent screen does not have the setting, turning a fact about Google's console into a
  claim in a manifest.
- **A `bool` for the assessment requirement.** Rejected: `NotRequired` and `NotEstablished` are different
  answers, and the second is the one that needs an action.

## Conditions that would justify revisiting

- The Calendar or OpenID scope categories are published — then the table gains entries and the connector's
  burden becomes a stated value rather than `Unestablished`.
- Google recategorises a scope, which the table's date and the record's Verification Log are designed to
  surface rather than absorb.
- A second connector family (Microsoft, GitHub) is added with a comparable review taxonomy, which would make
  the accounting a shared trait rather than a `google::scopes` function.
- A deployment records its consent-screen audience, which would make the internal-app exemption a fact rather
  than a condition this module cannot see.
