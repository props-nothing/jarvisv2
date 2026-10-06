# ADR-0018: An approval is bound to the exact arguments it was asked about

**Status:** Accepted (simplified by `ADR-0136`)

**Date:** 2026-09-22

## Context

A held tool call becomes a durable approval request (`P3-004`). What an owner says yes to must be exactly what runs:
if the arguments could change between the question and the answer, a yes would authorise something nobody read.

## Decision

1. An approval records the **tool, its version and a digest of the canonical arguments** (`CanonicalIntentHash`: a
   SHA-256 over the tool, version and arguments with object keys sorted, so the same call always hashes the same).
2. The call that runs after a yes is recomputed and compared with that digest; a different argument set is refused.
   A model that edits its request after asking therefore has to ask again.
3. An approval also records a human-readable preview, the risk level, an expiry and the instant it was decided; the
   decision itself is recorded by `ADR-0043`.
4. One approval exists per run and digest, so the same action is never put to the owner twice at once.

## Consequences

- Editing an action invalidates the approval, which is the property that makes "yes" meaningful.
- The digest is deterministic, so it is also usable as the key for a tool call's ledger (`ADR-0019`).