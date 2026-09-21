# ADR-0009: Migrate Prototype Behavior Clean-Room

- Status: Accepted
- Date: 2026-09-20

## Context

The repository's Python example demonstrates valuable behavior, but its README identifies CC BY-NC 4.0 licensing, while the new platform license is undecided. It is also a single-process prototype with plaintext credential storage and different trust boundaries.

## Decision

Use `example/` as a quarantined behavioral reference. Document observable invariants and reimplement them independently in the new architecture. Do not copy or mechanically translate source unless explicit legal review confirms compatibility and records attribution/obligations.

Freeze prototype changes except urgent security/data-loss fixes. Migrate user data only through an explicit previewed import tool that excludes secrets.

## Consequences

- Useful confirmation, undo, memory-budget, voice, visual, and outcome-honesty behavior remains in requirements/tests.
- Implementation takes longer than direct translation but avoids importing architectural and licensing debt.
- The example stays available until replacement acceptance evidence and user migration exist.
- Third-party assets and their licenses remain confined and tracked.

## Alternatives

- Port source line by line: risks license incompatibility and preserves the monolith.
- Delete the prototype now: loses hard-won behavioral knowledge and regression examples.
- Keep extending Python as the product: conflicts with the accepted control-plane/installability architecture.

## Revisit When

Specific source reuse may be approved file by file with owner permission/legal review, provenance, attribution, and an ADR or dependency record. The overall migration remains behavior-first.