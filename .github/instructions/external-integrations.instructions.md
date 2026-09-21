---
name: External Integration Evidence
description: "Use when adding or changing an external API, SDK, connector, model provider, agent runtime, MCP/ACP protocol, OAuth flow, webhook, voice provider, telephony integration, or platform service. Requires live official documentation and llms.txt research before code."
applyTo:
  - "crates/jarvis-connectors/**"
  - "crates/jarvis-models/**"
  - "crates/jarvis-runtimes/**"
  - "crates/jarvis-tools/**"
  - "crates/jarvis-voice/**"
  - "runtimes/**"
  - "extensions/**"
  - "installers/**"
---

# External Integration Evidence

- Before editing implementation, follow `docs/development/external-research.md`.
- Fetch the provider's live official `llms.txt` or section index when available, then the exact official API/spec/SDK/changelog pages for this operation.
- Create or refresh `docs/research/integrations/<integration>.md` with URLs, versions, access date, auth/scopes, limits, idempotency, retries, webhooks, privacy, deprecations, unresolved questions, and a falsifying test.
- If authoritative documentation cannot be reached, stop provider-specific implementation rather than guessing from memory.
- Keep provider types and credentials inside the adapter. Map effects, risk, scopes, errors, usage, events, and outcomes into canonical JARVIS types.
- Validate against official schemas or sanitized real wire fixtures and include an opt-in live smoke test where provider behavior cannot be proven offline.
- Never include secrets, personal data, signed URLs, real phone numbers, transcripts, or recordings in docs, logs, or fixtures.