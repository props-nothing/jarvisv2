# ADR-0140: Remembering is conversational, and a claim with no subject is about the owner

Status: Accepted
Date: 2026-10-06

## Context

Asked to "remember to reply with short sentences", the assistant answered that the system had rejected the entry because it
did not recognise an ID for the user. Two rules combined to make the most ordinary request impossible:

- A memory had to name an entity by identifier, and the model has no way to learn one. Nothing created a subject for the
  person who owns the profile.
- A model's proposal is stored as an unconfirmed model inference, and retrieval deliberately never reads those, so even a
  proposal that was accepted would never change a later answer. There was also no place to confirm one.

The proposal boundary is worth keeping: a page the assistant fetched could say "remember that the user wants X", and a model
that could write straight into trusted memory would be one prompt injection away from corrupting it.

## Decision

1. **A claim with no named subject is about the profile's owner.** The memory service resolves an empty `entity_ids` to one
   confirmed `Person` entity labelled `You`, created the first time it is needed and found again afterwards. This applies to
   `jarvis.memory.propose` (the field is now optional), `POST /api/v1/memories` and `jarvis memory remember`. A *named* entity
   must still exist in the workspace; an invented identifier is still refused by field. What changed is the refusal of the
   empty list, not the check that a named subject is real. An explicit empty array in a proposal is still malformed.
2. **The inference boundary is unchanged.** A model's proposal is still a `Proposed` claim from `model_inference` and is still
   never retrieved. The model still cannot choose a source, confidence, locator or supersession.
3. **The person keeps a proposal in the console.** Proposals appear in "Waiting for you" as "remember?" cards with Keep and
   Dismiss. Keep forgets the proposal (without the tombstone) and files the same text as the person's own `user_statement`, which
   retrieval does read; Dismiss forgets it and keeps the tombstone so it is not proposed again. The order matters: filing
   first is recognised as the claim already held, and nothing is saved. This is the light-approval shape of ADR-0136: one click, no ceremony, and the act is the
   person's, not the model's.

## Consequences

- "Remember that I like short answers" works end to end: the model proposes, the console offers it, Keep makes it shape later
  answers.
- A proposal is not yet answerable by voice or from the terminal in one step (`jarvis memory` can list and confirm). Spoken
  "keep that" is a later slice.
- The owner entity is created lazily, so a profile that never remembers anything has none.

## Falsification

Tests: two entity-less claims share one owner entity; an invented entity identifier is still refused with a `422` naming
`entity_ids`; a proposal with no subject is stored `Proposed` from `model_inference` against the owner; an explicit empty
`entity_ids` array is still refused.
