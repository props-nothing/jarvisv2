# ADR-0101: A missing control and a failed one are not the same answer

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — verifying the Calendar channel token, the only control on a
  bodyless delivery). Completes the "surfaced, not verified" limit `ADR-0099` and `ADR-0100` each recorded.
- **Relates to:** `ADR-0099` (the echoed-channel-token authenticator this verifies), `ADR-0100` (the Calendar
  message whose token this checks, and which recorded the limit), `ADR-0055` (the OAuth `state` is compared in
  constant time — the routine reused here), `ADR-0066` (a guard that can never fire), and `ADR-0094`'s "a
  refusal keeps the cursor" family: a control that distinguishes "absent" from "failed".

## Context

A Calendar notification-channel delivery has a **zero-length body** (`ADR-0100`), so there is **no MAC to
check** — the fact that made `ADR-0099` widen the webhook contract for a body-independent authenticator. That
leaves exactly **one** control on the path: `X-Goog-Channel-Token`, *"an arbitrary string value to use as a
channel token … you can use the token to verify that each incoming message is for a channel that your
application created"*. `google::channel` **read** it (`ADR-0100`) but deliberately did not **compare** it,
recording the comparison as `P5-010`'s work — so the connector could name an authenticator and parse its input
yet still not decide whether a delivery was authentic. That gap is what this slice closes.

Two facts about the token shape the decision, and both come straight off the page:

1. **It is optional.** The header is *"Sometimes present"* / *"Only present if defined"*, because the `token`
   property on the `watch` request is optional. So a delivery **without** a token is a documented shape, not a
   malformed one — but only for a channel the connector registered **without** a token.
2. **It is the only control.** Nothing else on the request authenticates it: the body is empty, and there is no
   signature. So the comparison's failure mode is the whole security posture of the path.

Those two facts pull in opposite directions for a `bool`, which is the finding: `matches(stored, delivery) ->
bool` would answer "did it match", and read as `false` for **both** "the channel was registered without a token,
so there is nothing to check" and "the channel is protected and this delivery failed the check". One is a
documented configuration; the other is a forged or misrouted delivery. A predicate that cannot tell them apart
makes a caller either **alert** on a channel the operator chose not to protect, or **accept** a delivery that
failed the one control it has.

## Decision

1. **`verify_channel_token` returns a four-variant `ChannelTokenCheck`, not a `bool`.** The four are two pairs,
   and **only one member of each pair is a refusal**:

   | variant | stored token | delivery carries | refusal? |
   | --- | --- | --- | --- |
   | `Verified` | yes | one that **matches** | no |
   | `Absent` | **no** | any | **no** — there is no control to check |
   | `TokenRequired` | yes | **none** | **yes** — it cannot prove the channel |
   | `Mismatch` | yes | one that does **not** match | **yes** |

   `may_be_acted_on()` is true for `Verified` and `Absent`; `is_rejection()` is its exact complement, named
   because a caller wiring an alert needs the negative reading. **`Absent` must not page anyone** — a channel
   registered without a token is a deliberate choice, the same "a replay must not page" rule
   `WebhookRejection::indicates_an_authenticity_failure` draws.

2. **The comparison is constant-time, and it reuses the crate's existing routine.** `SecretValue::matches`
   (`ADR-0055`) is used rather than `==`, because the stored token **is** a secret an attacker is trying to
   learn one byte at a time, and the OAuth `state` already had the correct comparison. One implementation, one
   place to audit — the reasoning `jarvis-core`'s `secretbytes` records for its own extraction.

3. **The candidate is compared **exactly** — no trimming, no case folding.** The token is a shared secret, so a
   prefix, a superstring, a case variant, or a whitespace-padded value is a **`Mismatch`**, each pinned by a
   test. Treating it as a case-insensitive identifier would accept a value the connector never registered.

4. **There is deliberately no separate length/bound check, and that is a decision rather than an omission.** The
   guide documents a maximum (`MAX_CHANNEL_TOKEN_BYTES` = 256, stated as a fact), but `SecretValue::matches`
   already refuses any candidate of a **different length** immediately and in constant time with respect to
   content — so no input can reach a walk of an attacker-sized value. A guard that rejected an over-long
   candidate "before the comparison" would be **redundant** *and* would itself scan the untrusted value: work
   with no refusal the comparison has not already made. The constant is therefore **documented, not enforced**,
   and a test asserts a value **at** the documented maximum verifies, so the absence of a hidden ceiling is
   observable rather than merely claimed.

## Consequences

- **The push path's only control is now a working control.** `verify_channel_token` decides authenticity for a
  mechanism whose delivery carries nothing else, so `ADR-0100`'s "surfaced, not verified" limit becomes
  "surfaced and verified". The two ADRs together are the full arc for one mechanism: `ADR-0099` *named* the
  authenticator, `ADR-0100` *read* its input, and this one *compares* it.
- **The `bool` is what would have lost the finding**, and that is the general shape: a predicate over two
  outcomes (`true`/`false`) cannot express a question with **three** — matched, no-control, failed. Reading the
  enum name `Absent` in a caller makes "this channel is unprotected" impossible to confuse with "this delivery
  was forged", which is the `ADR-0035` "a boolean standing for more than two situations is an enum" rule
  applied where the two collapsed states have **opposite operational readings**.
- **⚠ A guard in my own first draft could never *decide* anything, and the test for it could not tell.** The
  first version checked `presented.chars().count() > MAX_CHANNEL_TOKEN_CHARS` and returned `Mismatch` before
  comparing. But `SecretValue::matches` already early-returns on a length mismatch, so the guard changed **no
  input's answer** — every case it "caught" was a `Mismatch` anyway. Worse, the guard was an **unbounded `O(n)`
  walk of the attacker-supplied value**, i.e. it added exactly the work it claimed to prevent. This is
  `ADR-0066`'s family in a new form: not a guard that can never *fire*, but one that can never *decide* — and
  the **test I wrote for it could not separate the two behaviours**, so it passed under a mutation that removed
  the guard. The correction removed the guard, renamed the constant to say **bytes** and to say **stated, not
  enforced**, and rewrote the test to assert the observable property (an over-long candidate is a `Mismatch`
  *because its length differs*, and a value at the documented maximum **verifies**). **The general rule: before
  adding a check, ask which input it changes the answer for — if none, it is not a check but a cost.**
- **Two guards were falsified A-B-A with compiling mutants.** (a) The missing-token arm mutated from
  `TokenRequired` to `Absent` (reading a configured control's absence as "no control" — the fail-open
  direction); the test failed with *"a registered token with no presented one cannot prove the channel"*. (b)
  The comparison mutated to a case-insensitive `to_lowercase()` match; the exact-match test failed. Both
  restored byte-identically.
- **The bound is a fact, and facts that are not enforced are labelled as such.** `MAX_CHANNEL_TOKEN_BYTES` is a
  `pub` constant with the guide's figure, and its doc says plainly that **nothing enforces it and why**. This is
  the `ADR-0077` discipline ("a bound that is documented but not applied") turned inside out: here the honest
  statement is that the bound *needs* no application, and the reason is recorded so a later reader does not add
  the guard back.
- **A limit remains on the wider mechanism, and it is not this one.** The `EchoedChannelToken` **comparison**
  now exists; the **`OidcIdToken`** half — Google's Gmail Pub/Sub mechanism — still needs JWKS fetching, key
  rotation, and `aud`/`iss`/`exp` checking, which is unbuilt. So `docs/research/integrations/google.md`'s
  Unresolved Question 9 is **half closed** (the constant-time comparison) and **half open** (the JWT verifier),
  and the connector still declares `WebhookSupport::Polling` for the cardinality reason `ADR-0099` records —
  one `WebhookSupport` value against Google's two mechanisms.
- **Nothing yet calls `verify_channel_token`.** There is no delivery endpoint and no account/channel store, so
  this is a decision with tests rather than enforced behaviour — the "convention with tests, not a mechanism"
  limit this phase's slices each record.

## Alternatives considered

- **Return `bool`.** Rejected: it cannot express the three-outcome question, and the two `false` states have
  opposite readings — one must not alert, one must. This is the finding.
- **Return `Result<(), TokenFailure>`.** Rejected as the *primary* shape because `Absent` and `Verified` are
  both **non-refusals** and would have to share an `Ok(())`, re-collapsing the very distinction the enum
  carries; a caller could not tell "verified" from "no token configured" without inspecting the request again.
  The two refusal variants are still separately named, which is what a `Result`'s error half would have given.
- **Reuse `WebhookRejection` for the two refusals.** Rejected: `WebhookRejection` is the contract's vocabulary
  for a body-signature path (it has `InvalidSignature`, `Oversized`, `Replayed`, … that do not apply to a
  bodyless header token), and its `WrongAccount` is about the *account*, not the *channel*. The two refusals
  here are specific to this mechanism and are named for it.
- **Add a separate `MAX_CHANNEL_TOKEN_CHARS` guard on the candidate.** Rejected — this was the first draft and
  it is the finding above. It decided nothing and walked attacker input.
- **Compare with `==`.** Rejected: the stored token is the secret, and a short-circuiting comparison leaks its
  prefix. The crate already had the constant-time routine; using `==` would have been a second, weaker path.
- **Case-fold or trim the candidate before comparing.** Rejected: the token is a secret the connector chose, and
  exact comparison is the only rule that cannot accept a value it never registered. A "helpful" normalisation
  is an attack surface.
- **Refuse `Absent` (treat an un-tokened channel as unsafe).** Rejected: the guide makes the token optional, so
  this would refuse every delivery for a channel the operator configured exactly as documented — a control that
  fails closed on a correct configuration is not a control but an outage. `Absent` reports the fact; policy may
  still refuse it, but that is a caller's decision, not this function's.

## Conditions that would justify revisiting

- **A channel registration flow exists that records whether a token was set**, at which point the stored value's
  `None` is a recorded decision rather than a parameter, and `Absent` can cite that record instead of inferring
  from the `Option`.
- **A resource-identity check is added** (that the delivery names a resource this channel watches), which is the
  separate `security.md` "wrong endpoint" control this function explicitly does not perform, and which would
  sit beside it in the same decision.
- **The `OidcIdToken` verifier is built**, which would let the mechanism be chosen and the connector's `Polling`
  declaration be reconsidered — jointly with the `WebhookSupport` cardinality question `ADR-0099` and `ADR-0100`
  each name.
- **`WebhookSupport` gains a way to declare more than one mechanism**, at which point a Calendar push endpoint
  using `verify_channel_token` is declarable and the two-verifier asymmetry (one exists, one does not) becomes
  visible in the manifest.
