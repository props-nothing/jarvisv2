# ADR-0043: An approval decision records who answered and through which surface

**Status:** Accepted (simplified by `ADR-0136`)

**Date:** 2026-09-27

## Context

An approval is answered by a person, and the record of the answer should say who, when, how, and what they said, so a
tool call's receipt can cite it.

## Decision

A decision has four parts: the **outcome** (approve, deny, cancel), the **channel** it came through (`cli`, `desktop`,
`api`, `voice`), the **instant**, and the **approver**, a plain label for the owner who answered. The approver is
carried on the decision itself rather than passed beside it, so the record and the receipt cannot disagree about who
decided.

## Consequences

- A receipt for a call that ran after a yes names the approval that released it and who gave it.
- The same shape serves the other thing an owner decides, promoting a skill (`ADR-0117`).