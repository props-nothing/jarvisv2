# ADR-0008: Voice Is Provider-Neutral; ElevenLabs Is An Adapter

- Status: Accepted
- Date: 2026-09-20

## Context

JARVIS needs local conversation, inbound calls, and outbound calls. ElevenLabs offers strong speech, Custom LLM, MCP, Twilio, and SIP capabilities, but provider-owned identity, tools, memory, or workflow state would make the voice channel a second brain.

## Decision

Define provider-neutral voice, speech, session, and call ports. Prefer ElevenLabs Custom LLM mode in which `/v1/responses` or `/v1/chat/completions` routes into a scoped JARVIS run. Also support a least-privilege JARVIS MCP profile for specialized ElevenLabs-owned agents.

JARVIS owns identity resolution, policy, tools, memory, call records, consent/retention, and webhook reconciliation. ElevenLabs is the first adapter and remains replaceable.

## Consequences

- The same core supports local and hosted voice.
- OpenAI-compatible SSE and provider webhook contracts require adapter tests.
- Calls need short-lived identity/session binding and stricter authority than ordinary local chat.
- Telephony introduces legal, consent, recording, retention, latency, and retry requirements.
- Outbound calls are policy-controlled external effects, not arbitrary model actions.

## Alternatives

- ElevenLabs agent as canonical brain: duplicates memory/policy and couples JARVIS to one voice provider.
- JARVIS tools exposed broadly over MCP: violates least privilege.
- Build complete telephony/audio infrastructure first: delays product value and ignores mature providers.

## Revisit When

Provider preference may change. Canonical ownership and the provider-neutral ports remain.