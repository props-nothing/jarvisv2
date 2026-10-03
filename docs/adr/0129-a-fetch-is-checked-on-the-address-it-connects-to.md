# ADR-0129: A fetch is checked on the address it connects to

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P3-027`

## Context

Until now the model could read workspace files and propose memories, and nothing else. An agent that cannot read the
web is not much of an agent, but a tool that fetches a URL **the model chose** is the textbook server-side request
forgery primitive: the daemon runs on the user's machine, inside their network, next to a cloud metadata address, a
router, and every service bound to loopback. `docs/architecture/security.md` already requires "scheme/host/IP policy;
DNS rebinding defense; redirect revalidation; block metadata/private ranges by default".

There is also a second, quieter problem. A URL carries data to its destination. A page the model has just read can
contain text that asks for a fetch of `https://attacker.example/?q=<something private>`, and no address rule stops
that, because the address is public.

## Decision

**`jarvis.web.fetch` lives in a new adapter crate, `jarvis-web`, and is checked on the address it connects to.**

1. **The check is an allowlist of the global unicast space, applied to resolved addresses.** An IPv6 address is public
   only inside `2000::/3`; an IPv4 address is public unless the IANA special-purpose registry marks its block as not
   globally reachable, or it is multicast or reserved. IPv4-mapped, NAT64 and 6to4 addresses are judged by the IPv4
   address they embed. A name is resolved once, **every** answer must pass, and the connection is **pinned** to those
   answers (`resolve_to_addrs`), so a second lookup cannot return something else.
2. **Every redirect hop repeats the whole check.** reqwest's redirect following is disabled and the adapter follows
   at most three redirects itself.
3. **The request has no model-chosen degrees of freedom beyond the URL**: `GET` only, a fixed `User-Agent` and
   `Accept`, no cookies, no proxy, no decompression, `http`/`https` on ports 80/443 only, no credentials in the URL.
4. **The body is read as a bounded stream and the result is fenced untrusted text.** The adapter applies
   `IsolatedText` itself, so no caller can forget to.
5. **Declared `read_only`, risk 2, approval `policy`.** The default workspace holds risk 2 for a person, so by default
   the user sees the URL before it is requested. This is the answer to the exfiltration problem: it is not solvable by
   the tool, so it is decided by a person until the operator chooses otherwise. `ExternalCommunication` was rejected
   because it forces approval through a workspace flag the operator cannot relax, which would make the tool
   permanently non-autonomous; `WorkspacePolicy` can only tighten an approval (`ADR-0122`), so the safe default has to
   be the declared one. The opt-in for unattended use is `policy.approval_threshold = "high"`, which affects every
   risk-2 tool and is the operator's explicit trade.
6. **The loopback exception exists only in test builds** and names one port. A production `EgressPolicy` has no way to
   say "allow loopback", so neither configuration nor a model argument can ask for it.

## Consequences

- The model can read public pages. *(Amended by [ADR-0133](0133-approval-is-for-what-can-hurt-and-the-owner-can-decide-once.md): the fetch is risk 1 and runs by default; an `ask` override restores the hold.)*
- A model-chosen URL can still carry data out **after approval or opt-in**. This is recorded as the first unresolved
  question in `docs/research/integrations/web-fetch.md` and must not be described as solved.
- The model sees at most 4,000 characters of a page: the executor truncates every tool result to 12,000 characters
  (`jarvis_tools::MAX_MODEL_FACING_RESULT_CHARS`, raised from 4,000 by `P3-029`, which also made the fetch cap derive
  from it) and the fence must survive. An `offset` argument is the follow-up; the limit is asserted by
  `the_worst_escaping_page_still_fits_the_executors_result_budget`.
- `http` is allowed; the result reports the final URL so the scheme is visible.
- Non-UTF-8 charsets are decoded lossily.

## Falsification

Mutating the address rule to accept everything fails seven tests, including the discriminating one:
`a_refused_destination_receives_no_connection` asserts the loopback server saw **zero** connections, by address
literal and by the name `localhost`. `every_redirect_hop_is_checked_again` fails if a redirect to the metadata address
is attempted instead of refused. `one_private_answer_among_public_ones_refuses_the_name` covers the mixed-answer case
that DNS cannot be made to produce in a unit test.
