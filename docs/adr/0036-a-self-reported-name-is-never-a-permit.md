# ADR-0036: A self-reported name is evidence or nothing, and never a permit

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009c` adds a per-client allowlist and rate limits. An allowlist needs a **subject** — something an entry
names — and the obvious candidate is the `clientInfo` a caller sends in each request's `_meta`.

So the protocol's shape was read before the design. `Implementation` in `rmcp-3.4.0`'s `model.rs` is
`{name, title, version, description, icons, website_url}`, and `RequestMetaObject::client_info()` decodes it
from per-request metadata. **Every field is chosen by the client and nothing verifies any of them.**

An allowlist keyed on that would be a control a client names itself into: a caller that sends
`name = "vscode"` would be admitted **as VS Code**, with no credential at all. That is `ADR-0024`'s defect —
*a server does not name itself* — arriving on the inbound side, where it is worse, because the label now
decides **permission** rather than merely identity.

The protocol's own answer for a remote server is an OAuth token, and its rules are specific: a server **MUST**
validate that a token was issued **for it** as the intended audience (RFC 8707) and **MUST NOT** accept a token
meant for another resource. That is a slice, not a value — `auth` remains off in this workspace.

## Decision

**1. The allowlist's subject is a credential fingerprint, and a label is evidence.**

`CallerAdmission::decide` takes an `Option<&Fingerprint>` and nothing else. `CallerLabel` is recorded, bounded,
compared for **reporting**, and never consulted to permit. The rule is stated in the type's doc because it is
the module's reason to exist:

> **A self-reported name is evidence or nothing. It is never an identity, and never a permit.**

This mirrors `jarvis_mcp::ReportedIdentity`, which records what a server claims and refuses to use it as an
identifier. The same reasoning now holds in the other direction, so the crate has one rule facing both ways
rather than one facing each.

**2. A label mismatch is reported, never refused.**

`label_matches` returns `Option<bool>`: `Some(false)` for an admitted credential presenting a different label,
`None` for one that is not admitted (there is no recorded label to compare).

Refusing on a mismatch was the tempting alternative and it is wrong: the fingerprint already admitted the
caller, and refusing would make the label **a second permit after the module has said it is not one**. It would
also break every call from a valid credential the moment a client upgraded its version string. A mismatch is
worth *seeing* — an upgrade, or a client reused under a different identity — so it is reported and the operator
decides.

**3. A fingerprint is a digest, and a pasted credential is refused locally.**

`Fingerprint::parse` accepts only the digest alphabet (alphanumerics plus `+`, `/`, `=`) and rejects anything
else, so a JWT (`eyJ…eyJ…`) or a `Bearer …` value fails **in the configuration reader**. That is what stops the
operator error that would otherwise put a secret in a file that gets compared, formatted into diagnostics, held
for the process's life, and committed. The same reasoning as `ApiKey::new` rejecting a pasted URL.

`Display` prints a **prefix and a length** (`deadbeef… (32 chars)`), so a log line names which caller was
refused without putting a full digest where anything reading logs can collect them.

**4. An entry holds a fingerprint, never a credential.**

The same reasoning `jarvis-tools` applies to an approval nonce: this value is compared on every request,
formatted into error messages, and held for the process's life — the place least able to protect a bearer token.
A digest answers the only question the allowlist asks, which is "is this caller one we allow".

**5. `local_only` admits no remote caller, and the two empty states are named for the difference.**

`CallerAdmission::local_only()` produces an empty, **enforced** allowlist, so a **remote** caller is refused and a
**local** one is admitted. The accessor is `is_local_only()` rather than `admitted().is_empty()`, for the reason
`ServerExposure::is_loopback_only()` exists: an empty allowlist that is enforced and one that is not are
opposites, and a reader checking only for emptiness would conclude the reverse of what they hold.

This is the **second statement** of the same decision `ServingConfig` already makes for the bind, and the
redundancy is deliberate: a control that depends on another control having worked is not a control. If a proxy,
a forwarded socket, or a misconfiguration changed the bind, this is what refuses the caller.

> **CORRECTION (2026-09-23, `P3-009h`): the first version of this decision could not admit a local caller at
> all, and its own doc said the opposite.** `decide` took only a credential fingerprint, so `None` meant "no
> credential" and was refused — but a local caller is precisely the case the credential apparatus does not
> apply to. So `local_only` refused **everybody** while this ADR said "the only admitted caller is a local
> one". The fix is `CallerOrigin`, a value the **request layer** supplies and nothing a caller can send, and the
> falsification reproduces the bug exactly (`left: NoCredential, right: Admitted`).
>
> **The lesson is about what a test asserts.** The original tests asserted the *doc's* claim about a local
> caller nowhere — they checked that a remote caller is refused, which the bug satisfied. A shipped document
> claiming behaviour the code does not have is the same defect class this phase has now found five times, and
> the remedy is the same: write the test that asserts the doc's own words.

**5b. The rate limit applies to a local caller too.**

A runaway local client can monopolise the daemon as readily as a remote one, and exempting `Local` would make
the bound reachable by anyone who could reach the socket. So `decide` checks the budget **before** it checks the
origin, which is also why the origin cannot be an early return that skips it.

**6. Rate limiting is a bound here, and the counting is the daemon's.**

`decide` takes a `spent_budget: bool` and returns `RateLimited { requests_per_minute }`. This module decides
whether a caller has room; the daemon's request layer counts.

Putting the counting here would mean the value held **mutable state**, and a policy that mutates per request
cannot be compared, logged, or reused — which are exactly the properties that make it reviewable. The budget is
a **rate** (twelve per minute by default) rather than a total, so a client that bursts then idles is not punished
for the burst.

**7. The refusals answer `401`, and the reasons are for the operator.**

`NoCredential` and `NotAllowed` both answer `401`, not `403`. From the caller's side an absent credential and an
unrecognised one are the same situation — present a valid one — and distinguishing them would tell a prober
which fingerprints exist. `RateLimited` answers `429`. The variants exist for the log and the audit record, and
the message a caller receives is not the reason.

**8. An absent label is `absent()`, not a placeholder.**

A caller with no `clientInfo` records an **empty** name rather than `"unknown"`, because a placeholder is a value
a caller could also send — so `name = "unknown"` would be indistinguishable from silence, and telling those apart
is the only reason to record a label at all.

## Consequences

- **The inbound half of ADR-0024 now has a type.** A reviewer asking "can a caller talk its way in?" finds
  `CallerLabel`'s doc and `decide`'s signature, which takes no label.
- **Both central properties are pinned by falsification.** Removing the fingerprint check makes
  `a_claimed_label_does_not_admit_an_unlisted_credential` fail with `left: Admitted` — the impostor admitted —
  and three tests in total fail. Forcing `label_matches` to return `true` fails
  `a_label_mismatch_is_reported_rather_than_refused` with `left: Some(true)`.
- **A configuration error is caught where it happens.** A pasted token fails `Fingerprint::parse` with a message
  saying it is probably not a fingerprint, rather than being admitted or stored.
- Still **not built**, recorded rather than implied: **nothing binds**, so this policy is a value no request
  reaches yet — binding the endpoint in the daemon is the next slice, and the `Origin` decision plus this one are
  then applied at the request layer. The **token itself is unvalidated**: audience binding (RFC 8707) and
  Protected Resource Metadata (RFC 9728) are the OAuth slice, and until they exist the daemon admits **nobody
  remotely**, which is why `is_local_only` is the default. The clock is not modelled here — a budget window needs
  one, and that belongs with the counting.
- **The counting layer is a stub by construction.** `spent_budget` is supplied by the caller, so a daemon that
  never sets it has a rate limit that never fires. That is recorded rather than implied, because a bound nothing
  enforces reads exactly like a bound that works.

## Alternatives rejected

- **Key the allowlist on `clientInfo`.** A client names itself into the permit — `ADR-0024`'s defect on the
  inbound side.
- **Refuse on a label mismatch.** Makes the label a second permit, and breaks a valid credential on a client
  upgrade.
- **Store the credential rather than a fingerprint.** A bearer token in the value compared per request,
  formatted into diagnostics, and held for the process's life.
- **Accept any non-empty string as a fingerprint.** Loses the check that catches a pasted token.
- **Print the whole fingerprint in `Display`.** Puts full digests in every log line that names a caller.
- **Hold the rate-limit counter here.** A policy that mutates per request cannot be compared, logged, or reused.
- **A total budget rather than a rate.** Punishes a client's burst for the rest of the window, which is not what
  a bound on a runaway caller needs.
- **Distinguish "no credential" from "unknown credential" on the wire.** Tells a prober which fingerprints exist.
- **Use a placeholder for an absent label.** Makes a claimed `"unknown"` indistinguishable from silence.
- **Make the bind the only control.** A control that depends on another having worked is not a control.
