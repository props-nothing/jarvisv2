# ADR-0136: An approval is a yes or no from the owner

**Status:** Accepted

**Date:** 2026-10-06

## Context

JARVIS is a single-owner, local product whose promise is an assistant that does things and shows its work. Asking is
how it stays trustworthy, but every extra step in answering makes it less useful: a question that can only be answered
from a terminal, or that needs something fetched first, is a question people stop being asked, and then they turn the
protection off. Approval has to be as light as the question it asks.

## Decision

1. **Deciding whether to ask is policy; answering is the owner's.** The deterministic policy decides which calls are
   held: the risk threshold, per-tool `ask`/`deny` settings, the always-ask rule for tools that talk to other people, and
   `trust` for tools the owner has decided may simply run (`ADR-0017`, `ADR-0122`, `ADR-0133`). The model can request
   a tool and can never change that decision, and it has no tool for answering an approval.
2. **A held call becomes a pending approval showing what would run:** the tool, its risk and the exact arguments
   (`ADR-0130`), bound to those arguments (`ADR-0018`).
3. **The owner answers with a yes or a no, from anywhere that holds the local credential:** the **Approve** and **Deny**
   buttons in the console, `jarvis approvals approve|deny`, the inline prompt in `jarvis ask` and `jarvis chat`, or a
   spoken "yes" / "no" in the console (`ADR-0135`). The request is authenticated by the bearer credential alone; there
   is nothing else to fetch, copy or enter.
4. **A decision records the outcome, the surface it came through, the instant and a plain approver label** (`ADR-0043`),
   so the audit says who answered and how.
5. **An approval expires** (one hour by default), is withdrawn if its run is cancelled (`ADR-0013`), survives a daemon
   restart (`ADR-0130`), and a denial continues the run so the model can say nothing ran.

## What this deliberately does not do

It adds no second step, no second device, no per-channel gating of who may answer, and no distinction between how
sure the system is of who is answering. In a single-owner local product the holder of the local credential is the owner.
A future deployment with several people will record and require *who* answered; that is a decision for that product
(`P10`), made then, and not a reason to make today's yes heavier.

## Consequences

- Hands-free works end to end: JARVIS says what it wants to do, the owner says "yes", and the run continues.
- An answer is as reliable as the surface it came through; a spoken answer depends on the browser's recognition.
- Policy stays the place where caution lives: raise the threshold, mark a tool `ask`, or `deny` it, and the questions
  follow.