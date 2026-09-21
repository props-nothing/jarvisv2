# JARVIS Documentation

## Start Here

1. [Product requirements](product/requirements.md)
2. [Architecture overview](architecture/overview.md)
3. [Repository layout](architecture/repository-layout.md)
4. [Roadmap](../ROADMAP.md)
5. [Implementation backlog](../TODO.md)
6. [Glossary](glossary.md)

## Architecture

- [Runtime and models](architecture/runtime-and-models.md)
- [Tools, MCP, and connectors](architecture/tools-and-connectors.md)
- [Memory and context](architecture/memory-and-context.md)
- [Identity and workspaces](architecture/identity-and-workspaces.md)
- [Storage](architecture/storage.md)
- [Events and workflows](architecture/events-and-workflows.md)
- [Voice and telephony](architecture/voice-and-telephony.md)
- [Security and threat model](architecture/security.md)
- [Protocols](architecture/protocols.md)
- [Domain/API contracts](api/contracts.md)
- [Conceptual data schema](data/schema.md)

## Decisions

- [ADR index](adr/README.md)

## Engineering

- [Getting started](development/getting-started.md)
- [External integration research](development/external-research.md)
- [Evaluated prototypes](research/evaluated-prototypes.md) — what was built, measured, rejected, and where the findings live
- [Integration research records](research/integrations/README.md)
- [Testing strategy](development/testing.md)
- [Definition of done](development/definition-of-done.md)
- [Acceptance tests](quality/acceptance-tests.md)

## Operations

- [Installation and release](operations/install-and-release.md)
- [Observability and diagnostics](operations/observability.md)

## Research And Migration

- [Upstream architecture patterns](research/upstream-patterns.md)
- [Integration research index](research/integrations/README.md)
- [Python prototype migration](migration/python-prototype.md)
- [Third-party provenance](../THIRD_PARTY.md)

## Authority

When documents conflict:

1. An accepted or superseding ADR controls the decision it covers.
2. Product requirements control externally observable goals.
3. Subsystem architecture controls ownership and invariants.
4. `TODO.md` controls implementation order, not architecture.
5. Research records control dated facts about external systems but must be refreshed before implementation.

Update links and affected documents in the same change. Do not create a second "master spec" that drifts from this set.