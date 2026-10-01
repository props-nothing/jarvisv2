# ADR-0105: The documented remedy is two steps, and only the second was a type

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` / `P5-010` (the Google connector — recovering from a rejected credential). The first
  piece of the reauth lifecycle `P5-010` owns.
- **Relates to:** `ADR-0082` (one classifier per API — the `authError` classification this consumes),
  `ADR-0064` (the token exchange, whose `RefreshOutcome` this routes), `ADR-0076`/`ADR-0077` (a guidance
  variant that exists because a number alone loses the reason), `ADR-0103` (the composition shape this
  follows), and `tools-and-connectors.md`'s "reauth path that preserves account references without hiding lost
  scopes".

## Context

The crate had **every piece** of credential recovery and **no composition**:

- `RefreshOutcome` (`crate::auth`) already distinguishes a plain refresh from a rotation from an expiry from a
  revocation from a transient failure, and already answers `needs_user()` / `is_safe_to_retry()`.
- `ReauthReason` (`crate::health`) already names *why* a user must reconnect, and already separates `ScopeLoss`
  from `Revoked` from `ProviderRefused`.
- `crate::google::client::classify` already flags an `authError` as `RetryClass::Authentication`.
- `RetryGuidance::Reauthenticate` already says *do not retry; a user must reconnect*.

**Nothing joined a rejected call to a refresh attempt and then to a reauth.** A caller holding a `RetryDecision`
for an expired token had exactly one credential answer available — `Reauthenticate` — and the errors guide is
explicit that this is the **second** remedy, not the first:

> "To fix this error, **refresh the access token** using the long-lived refresh token. If you're using a client
> library, it automatically handles token refresh. **If this fails, direct the user through the OAuth flow**."

So the connector's own vocabulary rendered **the second half of a two-step remedy as the whole of it**. Read
literally, `Reauthenticate` tells a caller to send a user through a consent screen as the **first** response to
an **expired token** — the exact failure a silent refresh repairs. The first step had no type anywhere.

**Second finding: `authError` is two causes wearing one word.** The same page:

> "This error occurs when the access token you're using is either expired or invalid. **Missing authorization
> for the requested scopes can also cause this error.**"

So one `401` + `authError` is **an expired token** (refreshing fixes it) *or* **a scope the grant never had**
(only re-consenting fixes it). The refusal alone cannot tell them apart. **A refresh can** — and only because it
cannot grant a scope: a call refused with a *fresh* token is not a token problem at all. That is the guide's own
*"if this fails"* branch reached from the other direction, and it is why the decision needs a fact the refusal
does not carry.

## Decision

1. **`recover_from_call(decision, state) -> CallRecovery` composes the classified refusal with whether a
   refresh was already tried.** `CallRecovery` is `Refresh` (the documented first step), `Reauth { reason }`
   (the second), or `NotCredential` (this is not a credential problem and the ordinary path applies).

2. **`TokenState` is a two-variant type, not a `bool`, and it names what each state *licenses*.**
   `PossiblyStale` → a refresh may help. `JustRefreshed` → the token was refreshed *in response to this same
   failure* and refused again, so no refresh can help. The distinction is the entire basis of the two-step
   remedy, and a `bool` named `refreshed` would read as a fact about the token when the decision turns on a fact
   about the **attempt**.

3. **`JustRefreshed` + `authError` → `Reauth { reason: ScopeLoss }`.** With the token excluded by freshness,
   the guide's documented remaining cause is a missing scope. The reason named is the one the **documentation
   supports**, and `ProviderRefused` (a persistent client-level problem) is explicitly **not** distinguishable
   here — only a re-consent that fails again would tell them apart — so the code says so rather than guessing
   the worse-sounding cause.

4. **A non-`Authentication` refusal is `NotCredential`, for every token state.** Only an authentication refusal
   reaches a refresh: a `429`, a `403` and a `5xx` must not spend the token endpoint's budget on a token that is
   not the cause. `NotCredential` says *this module has an opinion and it is that the credential is not the
   problem* — different from "not considered".

5. **`recover_from_refresh(outcome) -> RefreshRecovery` maps each outcome to exactly one action, and `Rotated`
   is distinct from `Refreshed`.** `RetryCall` / `RetryCallAndStore` / `RetryRefresh` / `Reauth { reason }`. The
   rotation carries a **store obligation** the plain refresh does not, because a caller that treats a rotation
   as an ordinary refresh keeps using a refresh token the provider has already invalidated — and the *next*
   attempt then reads as a broken account rather than as a missed store. `RefreshOutcome::Rotated` was already
   its own variant; this is where it reaches a caller as an **action**.

6. **`Transient` maps to `RetryRefresh`, not `RetryCall`.** The one case where the next attempt is another
   *refresh*: retrying the *call* with a token that was never obtained fails identically, while retrying the
   *refresh* is what a rate-limited token endpoint eventually honours. The direction is the decision.

7. **The two reauth outcomes keep their distinct reasons** (`Expired` vs `Revoked`), because
   `tools-and-connectors.md` requires a reauth path that does not hide what happened, and the two call for
   different user-facing explanations even when the same action fixes them.

## Consequences

- **The documented two-step remedy is now walkable as a sequence.** A test drives it end to end: a refusal asks
  to refresh (`CallRecovery::Refresh`); a failed refresh asks to reconnect with `Expired`; a successful rotation
  asks to retry **and store**; and a refresh that succeeds but is refused again asks to reconnect with
  `ScopeLoss`. Before this module the **first step had no value**, so the sequence was not expressible at all.
- **⭐ The finding's general form: a vocabulary that names the *last* remedy of a documented sequence makes the
  earlier steps unrepresentable.** `Reauthenticate` was correct about what to do *eventually* and wrong as the
  *first* answer — the `ADR-0096` "a comment discharged the work the code had not done" shape, in an enum: a
  value that *sounds* like the complete remedy and is only its tail. Ask of a remedy enum: **does it cover the
  sequence the provider documents, or only its end?**
- **⭐ And the corollary, which is why the state is an input: one error code can mean two causes, and the
  provider's own prescribed *first action* is what distinguishes them.** `authError` cannot be split from the
  refusal; it is split by **doing** the refresh. So the decision takes `TokenState` rather than pretending the
  `RetryDecision` is sufficient — the same inference/decision split the cursor and pub/sub decisions use.
- **Two guards were falsified A-B-A with compiling mutants.** (a) The stale-token arm mutated from `Refresh` to
  `Reauth { Expired }` — i.e. sending a user to re-consent for an expired token, the exact defect the slice is
  about; caught by the refresh-first test. (b) The rotation arm mutated from `RetryCallAndStore` to
  `RetryCall` — losing the store obligation; caught by the rotation test. Both restored byte-identically.
- **The research record is sharpened, not corrected.** Its `authError` line already quoted *"an expired or
  invalid token, or a missing scope"*; a log row now records the **fix-order** sentence verbatim, because that
  is the sentence the connector's vocabulary was missing, and it is what grounds decisions 1–3.
- **A limit remains: nothing calls these functions, and nothing performs a refresh.** There is no credential
  store and no token-refresh loop, so both are decisions with tests rather than enforced behaviour — the
  "convention with tests, not a mechanism" limit every slice of this connector carries. In particular the
  `Rotated`→`RetryCallAndStore` obligation is only **expressed**; a store that could discharge it does not
  exist. Nor is `ProviderRefused` reachable from `recover_from_call` (only from a caller that knows the client
  itself was refused), which is deliberate: this function cannot see that.

## Alternatives considered

- **Have `classify` return `Refresh` instead of `Reauthenticate` for a `401`.** Rejected: `classify` sees a
  response, not the token's age, so it cannot know whether a refresh would help — and `Reauthenticate` is the
  **correct terminal answer** once refreshing has failed. Changing it would move a decision that needs an input
  `classify` does not have into a function that cannot make it. The composition is the right place.
- **Decide from the `RetryDecision` alone (no `TokenState`).** Rejected: it cannot tell "the token is stale"
  from "the token is fresh and the scope is missing", so it would either always refresh (looping on a scope
  problem forever) or always reauth (the defect this ADR is about). The state is the missing input, and it is
  why the guide has two steps rather than one.
- **Return `ReauthReason::ProviderRefused` for the `JustRefreshed` case.** Rejected: the guide's remaining
  documented cause is a missing scope, and a persistent client-level problem is a *possibility* the refusal does
  not establish. Naming the graver cause would be asserting something this function cannot know — the
  `P3-008i` "do not assert a cause you cannot know" rule.
- **Fold `Refreshed` and `Rotated` into one "retry the call" action.** Rejected — this is the mutant the
  rotation test kills. The store obligation is the whole reason `Rotated` is its own `RefreshOutcome`, and a
  caller that misses it breaks the **next** run, not this one.
- **Map `Transient` to `RetryCall` (retry the original request).** Rejected: no token was obtained, so the call
  fails identically; the retry belongs to the *refresh*. This is the one place the direction is non-obvious and
  is why the variant is named `RetryRefresh` rather than left to the caller.
- **A single `Recovery` enum covering both call and refresh decisions.** Rejected: the two decisions consume
  different inputs (a classified refusal + state, versus a refresh outcome) and are taken at different points,
  so one enum would carry variants unreachable from either function — the "a variant nothing constructs"
  defect. Two types, each fully reachable from its own input.
- **Return a `bool` "recoverable".** Rejected: it erases which of refresh/reauth/not-credential applies and
  loses the reason a user must be shown, which the requirement explicitly forbids hiding.

## Conditions that would justify revisiting

- **A credential store and a refresh loop exist**, which is where `recover_from_call` is *used*, where the
  `Rotated` store obligation is **discharged** rather than merely expressed, and where `TokenState::JustRefreshed`
  is produced by the loop rather than by a caller.
- **`ProviderRefused` gains a producer** (a signal that the client identifier or the registration itself was
  refused), at which point the `JustRefreshed` arm could distinguish it from a scope loss and the recorded
  "not distinguishable here" note would become obsolete.
- **Google changes the `authError` causes or its documented fix-order**, at which point decisions 1–3 are
  revisited against the new text — the whole slice is grounded on that one sentence.
- **A second provider's errors need the same composition** (`P5-006` Microsoft), at which point the
  call/refresh recovery shape is reconsidered for sharing — and, as with the ingest types (`ADR-0104`), the
  shared shape would have to carry each provider's **remedy sequence** rather than assume it, or it would repeat
  the mistake this ADR corrects.
