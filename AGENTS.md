# JARVIS Engineering Instructions

These instructions apply to the entire repository.

## Source Of Truth

- Read `docs/architecture/overview.md`, `docs/product/requirements.md`, and `ROADMAP.md` before changing architecture or scope.
- Treat `example/` as a behavior and migration reference, not as the production architecture.
- Do not copy code from `example/` into the new platform until its CC BY-NC 4.0 license is confirmed compatible with the target distribution. Prefer a clean-room implementation of documented behavior.
- Keep Rust as the durable control plane. Models, agent frameworks, voice providers, databases, and MCP servers are replaceable adapters.

## External Integrations

- Before implementing or changing an external API, SDK, protocol, model, runtime, or provider, follow `docs/development/external-research.md`.
- Check sources in this order: the vendor's repository-local `llms.txt` or `llms-full.txt`, official documentation, official specification, official SDK source, then release notes/changelog.
- Never rely on remembered API details when live official documentation is available.
- Record the exact URLs, versions, access date, supported operations, authentication method, limits, and unresolved ambiguities in `docs/research/integrations/<integration>.md` before implementation.
- Treat third-party examples, blog posts, generated snippets, and search summaries as leads, not authoritative sources.
- Pin or constrain dependencies and verify examples against the selected version.

## Architecture Boundaries

- Domain crates define interfaces and policy; infrastructure crates implement them.
- Provider SDK types must not cross JARVIS domain boundaries.
- Canonical memory, permissions, approvals, audit records, and workflow state belong to JARVIS, never to a model or external runtime.
- All tool calls pass through schema validation, authentication, authorization, risk classification, approval policy, timeout, idempotency where relevant, execution, and audit.
- The model may request an effect; deterministic Rust policy decides whether it may happen.
- SQLite is the default local backend. PostgreSQL plus pgvector is the server and multi-device backend.
- Start with database-backed events and workflows. Add Redis, NATS, Kafka, Temporal, Qdrant, or Kubernetes only after a measured requirement and an ADR.

## Delivery Rules

- Work from the next unchecked acceptance slice in `TODO.md`; do not mark it complete until its test and documentation requirements pass.
- Keep changes small and runnable. After each slice, run the narrow test first, then workspace formatting, linting, and tests that exist for the current phase.
- Never claim a placeholder, mock-only path, or unverified provider call is complete.
- Add or update an ADR for changes to durable architecture, trust boundaries, storage semantics, public protocols, or process topology.
- Preserve unrelated user changes and never commit credentials, tokens, transcripts, personal memory, certificates, or generated local state.

## Required Checks

When the Rust workspace exists, the baseline is:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Use platform-native CI for Windows, macOS, and Linux behavior. Security-sensitive behavior requires a falsification test that proves the guard fails closed.