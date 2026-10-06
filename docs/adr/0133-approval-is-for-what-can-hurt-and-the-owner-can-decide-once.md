# ADR-0133: Approval is for what can hurt, and the owner can decide once

Status: Accepted
Date: 2026-10-03
Amends: [ADR-0129](0129-a-fetch-is-checked-on-the-address-it-connects-to.md), [ADR-0131](0131-model-authored-code-runs-in-a-disposable-container.md). Narrows [ADR-0122](0122-an-approval-override-can-only-tighten.md) in one stated way.

## Context

Using the product showed what the approval rules cost. Every web page the model wanted to read was held for a
person (the fetch was risk 2, the default threshold), and every snippet of code was held for ever (`Ask`, risk 3, which
no threshold can lift). An assistant that asks before a lookup or a calculation is one its owner stops using, and an
owner who learns to press "approve" without reading has less protection than one who is asked rarely and for reasons.
The goal of approval is to stand between the model and **effects that can hurt**, not between it and thinking.

## Decision

1. **A web fetch is risk 1 and runs under the default workspace.** It changes nothing, the URL is capped at 2,048
   characters (which bounds what one call can carry out), the address rule keeps it off private networks, the result
   is fenced as untrusted text, and every call is audited with its URL. The residual risk, data leaving in a URL the
   model chose, is accepted and stated in `docs/architecture/security.md`; an operator who does not accept it writes
   `"jarvis.web.fetch" = "ask"` under `[policy.approval]` and the old behaviour returns.
2. **The owner may trust a tool in advance:** `[policy] trust = ["jarvis.code.run"]` (`WorkspacePolicy::trusting`). It is
   the one setting that relaxes a tool's declaration, and it is deliberately narrow:
   - it waives only the tool's own `Ask` and the risk threshold, so a voice or scheduled run is not parked for a
     question the owner already answered;
   - it does **not** waive the `max_risk` ceiling, a denial, the actor's scopes, or the external-communication rule,
     and it is never consulted for a tool with an external-communication effect — trusting a mail tool changes nothing;
   - a denial or an approval override for the same tool **wins**, in either order of declaration;
   - nothing is trusted by default, and the setting names tools, so it cannot be widened by accident.
3. **`jarvis init` can set it up:** `--code-image IMAGE [--code-interpreter "node -e"] [--trust-code]`. Without
   `--trust-code` the default stays a question.

## Why code may be trusted at all

The code tool's container has no network, a read-only root, no files in or out, dropped capabilities, a process and
memory ceiling, and no state between runs (`ADR-0128`, `ADR-0131`). What a snippet can do is bounded by the
sandbox, so the question "may this run" is one an owner can answer once. Approval remains the answer for anything
the sandbox does not contain. `ADR-0131` decision 4 now reads: *no threshold makes code run unattended; only the
owner's explicit trust does.*

## Consequences

- The default experience asks far less: lookups run, and an owner who sets up code with `--trust-code` is not asked
  for calculations. Mail, calendars and anything with an external effect are asked as before.
- The pure policy engine gained one input and four limits, each with a falsification test
  (`standing_trust_waives_the_ask_and_nothing_else`, `a_trusted_code_tool_runs_without_a_hold`,
  `a_configured_trust_reaches_the_policy_and_tightening_outranks_it`).
- Not done: trusting from the approval prompt ("always allow"), which would need a writable policy (`P3-032`).