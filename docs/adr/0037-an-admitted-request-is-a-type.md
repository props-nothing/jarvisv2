# ADR-0037: An admitted request is a type, and the check order is a decision

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009a` decided which browser origins may reach JARVIS and `P3-009g` decided which callers may. Each closed by
recording the same honest limit: **nothing enforces them.** A policy is a control only where it is consulted, and
"consulted somewhere in the daemon" is how a pair becomes a single check nobody notices is missing — the failure
`P3-009f` had just demonstrated one level up, where a documented boundary was violated for two phases because no
test held it.

Two things therefore needed stating rather than leaving to a caller:

- that **both** policies were applied, not one;
- in **what order**, because a pair of checks has an order whether or not anyone chose it.

## Decision

**1. `RequestGate` is the one place a request is admitted, and it consults both policies.**

It holds a `ServingConfig` and a `CallerAdmission`, and `decide` returns either a refusal or an
[`RequestAdmission`]. There is no way to hold one policy and call it a gate, which is the property that makes
"both were applied" structural.

**2. An admitted request is a type with private fields and no public constructor.**

The only way to obtain a `RequestAdmission` is `decide` returning `Ok`. So a function taking one cannot be handed
a value that skipped a check, because none can be built.

A `bool` carries the same information and none of the guarantee: the caller decides what `true` means, and `true`
is also what a default-initialized field holds. This is the reasoning `P3-009e` used for the served set — a
catalogue is not a control — applied to admission rather than to tools.

**3. `Origin` is checked before admission, and that order is the specification's.**

The revision says servers **MUST** validate `Origin` on **all** incoming connections, so a hostile origin must be
refused whether or not the caller would otherwise have been admitted. Reversing the order would answer a request
with a hostile origin `401` — telling the caller about its credential while the origin was never examined — and a
request failing both would be reported as an admission problem.

The consequence is pinned: **a request that fails both is reported as an origin refusal.** Falsifying this by
swapping the two blocks fails `a_hostile_origin_is_refused_before_the_credential_is_considered` **and nothing
else**, which is what makes that test the one that holds the rule.

**4. The refusals carry different statuses, because they carry different remedies.**

`403` for an `Origin` tells a browser to stop, which is what the specification requires. `401` and `429` describe
something about the caller a client can act on. Folding them into one status would lose whichever remedy the
refused caller needs.

**5. `RequestAdmission` carries *how* the origin was decided, not only that it was.**

`OriginVerdict::Absent` and `Allowed` are different justifications for the same outcome. An audit record that
could not tell them apart could not answer "was this a browser request", which is the question a local-only
deployment most needs answered.

**6. The gate takes values, never a request.**

`decide(origin: Option<&str>, caller: CallerOrigin, credential: Option<&Fingerprint>, spent_budget: bool)`. A gate
that read a `HeaderMap` could not be tested without one, and deriving those values is the daemon's because it owns
the listener. Keeping this a function of its arguments is what makes the order and the pair testable at all.

**7. No consistency check between the two policies.**

A gate over a loopback-only `ServingConfig` and an admission policy that admits remote callers looks contradictory
and is not. The bind makes a remote caller unable to arrive; the admission policy refuses one that somehow does. A
reverse proxy, a forwarded socket, or a changed bind are each enough for the pair to be doing different work —
which is exactly the defence in depth `P3-009g` argues for when it says **a control that depends on another
control having worked is not a control.** Refusing that combination would remove the second control.

## Consequences

- **The limit every `P3-009` slice recorded is closed**: both decisions are applied to one request, in one place,
  with the order stated and pinned. What remains for the daemon is deriving the four values and answering the
  status — not deciding anything.
- **Two bugs in this slice were found by its own tests rather than by review.** The diagnostic helper
  `also_refused_admission` returned `false` whenever the allowlist was `local_only`, conflating "the allowlist is
  empty" with "this request carried nothing to check" — the same confusion `decide` had before `CallerOrigin`
  existed, reproduced within one commit of being fixed. Its test failed immediately.
  And the first fixture used `https://jarvis.example.com` as an allowed origin, which `ServingConfig::new`
  correctly refuses, so the fixture had to change to a loopback origin — the check working one layer up.
- **The falsification needed two attempts, and the first was wrong.** Disabling the admission refusal *and*
  swapping the order left the order test passing, because the test's request was never refused on admission. That
  is a reminder about what a falsification proves: the first attempt tested "the pair is broken", not "the order is
  reversed", and only the second isolates the property the test names.
- Still **not built**, recorded rather than implied: **nothing binds**, so no request reaches this gate — deriving
  the four values from a real request and answering the status is the daemon's, and the `spent_budget` flag still
  has no counter behind it. The token itself remains unvalidated (RFC 8707 audience binding, RFC 9728 metadata),
  so a remote caller can only be admitted against a fingerprint an operator configured by hand.

## Alternatives rejected

- **Return `bool` from the gate.** The caller decides what `true` means, and a default-initialized field is `true`.
- **Two functions, one per policy, for a caller to compose.** That is the shape that produced this slice: one of
  them gets forgotten, and nothing fails.
- **Check admission first.** Answers a hostile-origin request `401`, and the origin is never examined.
- **Return one status for every refusal.** Loses the remedy the refused caller needs.
- **Carry only that the origin passed.** Loses the distinction between a request from a browser page and one from
  a local tool, which is what a local-only deployment most needs in its audit record.
- **Refuse a gate whose two policies appear inconsistent.** Removes the defence in depth, on the reasoning that
  one control having worked means the other need not.
- **Take an `http::Request`.** Untestable without a listener, and it would put header parsing in the policy.

## Conditions that would justify revisiting

- **A second request shape reaching the same policies** (a stdio server path, or a long-lived stream whose
  admission is decided once). The gate would then need either a second entry point or an admission that outlives a
  single request, and "one request, one decision" is the assumption that would have to be re-argued.
- **The specification changing the `Origin` requirement to be conditional on the credential.** That would reverse
  the check order and the both-fail answer, and both are pinned by a test named for them.
- **A real rate-limit counter arriving.** `spent_budget` is a `bool` supplied by the caller; if counting moves into
  this crate the flag becomes a value, and the gate would be reading state rather than being told it.
