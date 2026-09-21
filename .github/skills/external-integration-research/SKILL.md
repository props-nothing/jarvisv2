---
name: external-integration-research
description: "Research a third-party integration before implementation. Use when adding or upgrading APIs, SDKs, connectors, model providers, MCP/ACP, runtimes, OAuth, webhooks, voice, telephony, Tauri, Temporal, or OS services. Finds official llms.txt/docs/specs, records evidence, and defines falsifying tests."
---

# External Integration Research

Use this skill before provider-specific code changes.

## Inputs

- integration/provider name
- exact operations being built
- intended deployment mode and language
- known version constraints

If these are not explicit, infer only from the selected `TODO.md` task and state the narrow scope.

## Workflow

1. Read `AGENTS.md` and `docs/development/external-research.md`.
2. Check `docs/research/integrations/README.md` and the existing provider record.
3. Fetch the live official `llms.txt`/section index, exact official pages, versioned spec or OpenAPI/AsyncAPI, official SDK source, and changelog.
4. Record facts separately from JARVIS decisions. Resolve contradictions through the most normative current source; leave unresolved conflicts explicit.
5. Map auth, schemas, streaming, pagination, limits, retries, idempotency, webhooks, privacy, and deprecations to JARVIS ports and policy.
6. Name the cheapest test that can disprove the central contract assumption and list contract/live tests.
7. Create or update `docs/research/integrations/<integration>.md` using `_template.md`; update the index.
8. Only then proceed to a small adapter edit and focused validation.

## Required Output

The record must contain:

- `last_verified` date and selected versions
- exact official URLs
- verified wire/auth/failure contract
- JARVIS mapping and risk/effect/scopes
- adopted and rejected choices
- verification plan
- unresolved questions and what they block

Do not claim the integration works until contract and live evidence required by its TODO item pass.