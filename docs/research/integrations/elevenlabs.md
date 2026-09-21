---
integration: elevenlabs
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: current hosted API as of verification date
selected_sdk: undecided
---

# ElevenLabs

## Scope

Architecture research for JARVIS-brain Custom LLM mode (both transports), JARVIS-as-MCP-server mode, inbound/outbound telephony, SIP, turn-taking configuration, and post-call event reconciliation. No SDK or account plan is selected, and no live call was made.

The 2026-09-21 refresh added the Speech Engine transport and the turn-taking configuration surface. Both were absent from the first pass, which read only the SSE compatibility pages.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Documentation index | https://elevenlabs.io/docs/llms.txt | 2026-09-20 | official page/API discovery and AI-agent instructions |
| Custom LLM | https://elevenlabs.io/docs/eleven-agents/customization/llm/custom-llm.md | 2026-09-20 | compatible endpoints, SSE, tools, extra body |
| Custom LLM via Twilio | https://elevenlabs.io/docs/eleven-agents/phone-numbers/twilio-integration/custom-llm-integration.md | 2026-09-21 | Speech Engine bridge: WS brain server, μ-law, signed URL, shared secret |
| Speech Engine | https://elevenlabs.io/docs/api-reference/speech-engine/speech-engine-upstream.md | 2026-09-21 | brain WebSocket contract and formats |
| Conversation flow | https://elevenlabs.io/docs/eleven-agents/customization/conversation-flow.md | 2026-09-21 | turn timeout, soft timeout, turn eagerness, max duration, interruptions |
| MCP integration | https://elevenlabs.io/docs/eleven-agents/customization/tools/mcp.md | 2026-09-20 | transports, approval modes, restrictions |
| SIP trunking | https://elevenlabs.io/docs/eleven-agents/phone-numbers/sip-trunking.md | 2026-09-20 | inbound/outbound SIP and security |
| Twilio outbound call | https://elevenlabs.io/docs/eleven-agents/api-reference/integrations/twilio/outbound-call.md | 2026-09-20 | endpoint and payload |
| Post-call webhooks | https://elevenlabs.io/docs/eleven-agents/workflows/post-call-webhooks.md | 2026-09-20 | HMAC, events, retry and payload behavior |
| OpenAPI | https://elevenlabs.io/docs/openapi.json | 2026-09-20 | machine-readable HTTP contract; not yet vendored/generated |
| AsyncAPI | https://elevenlabs.io/docs/asyncapi.json | 2026-09-20 | machine-readable WebSocket contract; not yet vendored/generated |

The docs index instructs agents to append `.md` for clean Markdown, use section `llms.txt` indexes, and exposes an official docs MCP server. Implementation still uses the exact product/API pages and schemas rather than the index summary alone.

## Verified Contract

### Custom LLM

- Supports OpenAI-style `POST /v1/chat/completions` and `POST /v1/responses` custom endpoints.
- Both stream with `Content-Type: text/event-stream`.
- Chat Completions chunks use `data: <json>\n\n` and finish with `data: [DONE]\n\n`.
- Responses requires at least `response.output_text.delta` and `response.completed` events, then `[DONE]`.
- Requests can contain standard OpenAI-format function tools for ElevenLabs system operations.
- Optional custom values arrive under `elevenlabs_extra_body`; these are caller/provider input and require an allowlist.
- Reasoning summaries, when enabled, are separate from final output. JARVIS must not expose hidden chain-of-thought.

### Speech Engine Transport (second custom-brain path)

Verified 2026-09-21. This is an alternative to the OpenAI-compatible endpoints, not a layer on them. Choosing one per deployment is required; running both against one number produces two brain sessions for one call.

- The brain runs on a **WebSocket server owned by JARVIS**. ElevenLabs connects to it to deliver transcripts and receive generated text.
- A **conversation WebSocket** is hosted by ElevenLabs; the telephony bridge connects to it using a **per-call signed URL**, so the bridge never holds the raw API key.
- The brain endpoint is protected by a **shared secret sent as a request header on the WS upgrade**. Without it, anyone who learns the URL can impersonate the provider.
- Audio is `ulaw_8000` in both directions. Twilio Media Streams speaks the same format, so a Twilio bridge relays base64 payloads with no transcoding.
- Observed event names on the conversation socket: `conversation_initiation_client_data` (bridge→provider, sent on stream start), `user_audio_chunk` (bridge→provider), `audio` (provider→bridge), `interruption` (provider→bridge), `ping`/`pong` keepalive.
- Provider `interruption` maps to a transport-level buffer clear. That is a playback action, not evidence about caller intent, and must not be recorded as an interruption by the caller.
- Documented trade-off: one WebSocket instead of a new HTTP connection per turn, described as a possible latency improvement. It is not a measured figure.
- Documented cost: the bridge adds two network hops on top of model time-to-first-token.
- Documented risk to carry into JARVIS: spoken input from a phone call is untrusted user input and must be validated before it influences tool calls or writes. This matches the existing JARVIS requirement that provider input is untrusted data.

### Turn-Taking Configuration (verified 2026-09-21)

These settings change what the caller hears and are part of JARVIS voice-session policy, not adapter internals. See [ADR-0010](../../adr/0010-model-based-turn-detection.md).

| Setting | Range / Values | Default | Notes |
| --- | --- | --- | --- |
| `conversation_config.turn.turn_timeout` | 1–30 s | — | wait during caller silence before taking the next turn |
| `conversation_config.turn.turn_eagerness` | `patient` \| `normal` \| `eager` | — | how quickly the assistant treats a pause as a turn opportunity |
| `conversation_config.turn.soft_timeout_config.timeout_seconds` | 0.5–8.0 | `-1` (disabled) | filler spoken while a slow reply is generated; recommended 3.0 |
| `conversation_config.turn.soft_timeout_config.message` | 1–200 chars | `"Hhmmmm...yeah."` | static filler; may instead be LLM-generated with a static fallback |
| `conversation_config.conversation.max_duration_seconds` | 60–7,200 | 600 | whole-conversation limit, independent of turn timeouts |

- Interruptions are a client-event selection: enabled, or disabled when complete delivery matters (disclaimers, safety instructions).
- The provider reports that the assistant may begin speaking after receiving enough words and a comma rather than a complete sentence. That means a **reply can be committed before its final clause exists**; JARVIS must not treat the first streamed segment as the complete response.
- The provider describes a **proprietary turn-taking model**. Unlike the turn settings above, it is not inspectable or swappable, so the effective boundary cannot be reproduced outside the provider. Record the provider's reported values as evidence and record the caller-interaction consequence in the call's latency report.
- WhatsApp message conversations have a separate default 15-minute inactivity timeout.

### MCP Consumer Mode

- ElevenLabs agents can connect to external MCP servers over SSE and Streamable HTTP.
- Workspace MCP use is opt-in and disabled by default.
- Server- and tool-level approval modes include always ask, fine-grained, and no approval.
- MCP is currently unavailable for Zero Retention Mode or HIPAA-required workspaces.
- JARVIS cannot delegate its final policy decision to ElevenLabs approval settings.

### Twilio Outbound Call

- `POST https://api.elevenlabs.io/v1/convai/twilio/outbound-call`.
- Required fields: `agent_id`, `agent_phone_number_id`, and `to_number`.
- Optional conversation initiation data includes dynamic variables, a user ID, custom LLM extra body, and bounded configuration overrides.
- Success response may include `conversation_id` and `callSid`.
- Provider acceptance does not prove a person answered; later events reconcile outcome.

### SIP

- Supports inbound and outbound calls through an existing SIP trunk.
- Authentication can use digest credentials or source-IP ACL; docs recommend digest over IP-only allowlisting.
- Production should use TLS 1.2+ signaling and SRTP media encryption when supported; UDP signaling is documented as experimental.
- Audio uses G711 8 kHz or G722 16 kHz at the SIP boundary.
- Custom inbound `X-` headers become dynamic variables and are untrusted context.
- Static-IP SIP endpoints are an enterprise feature; distributed/default endpoints may use changing source IPs.

### Post-Call Webhooks

- Event families include `post_call_transcription`, `post_call_audio`, and `call_initiation_failure`.
- Validate HMAC from the `ElevenLabs-Signature` header against the raw body and shared secret; official SDK helpers also validate timestamp.
- Audio callbacks can use chunked transfer and contain base64 MP3; they can be large.
- Repeated failures can disable a webhook; successful handlers return HTTP 200 promptly.
- Call-initiation failure metadata differs for Twilio and SIP.
- Events can be retried, so durable deduplication is required.

## JARVIS Mapping

- Preferred mode: ElevenLabs transports audio/telephony while `/v1/responses` creates a scoped JARVIS run.
- Compatibility mode: `/v1/chat/completions` maps to the same application command.
- Alternative brain transport: the Speech Engine brain WebSocket maps to the same application command as the HTTP endpoints, selected per deployment and recorded on the session.
- Turn and interruption policy map into the voice-session contract; the provider's effective values are stored as evidence next to the requested ones.
- Provider `interruption` maps to a transport clear, not to a caller-interruption event.
- Specialized mode: an ElevenLabs-owned agent gets a dedicated MCP client identity and explicit tool allowlist.
- Provider IDs map to `voice_sessions` and `voice_calls`; they never replace JARVIS IDs.
- `voice.call.start/status/end` are risk-classified JARVIS tools.
- ElevenLabs system tools map to a narrow voice-control adapter; general JARVIS tools remain behind policy.
- API keys and webhook secrets live behind `SecretRef`.
- Transcript/audio storage follows JARVIS retention and user policy; absence from JARVIS does not imply provider deletion.

## Decisions

1. Implement Responses compatibility before Chat Completions because it is the newer supported format.
2. Keep both endpoints behind voice-session authentication rather than exposing a generic unauthenticated model proxy.
3. Treat Custom LLM mode as the default personal-JARVIS voice architecture.
4. Keep MCP mode optional and least privilege.
5. Process verified webhooks through the durable event inbox.
6. Do not enable call recording or audio webhooks by default.
7. Implement the HTTP path first and treat Speech Engine as a later transport behind the same application command, because it adds a bridge process and a second credential (shared secret) without changing JARVIS ownership.
8. Set turn-taking policy explicitly per session rather than relying on provider defaults, and persist the effective values.
9. Treat provider `interruption` frames as transport control only; caller interruption is inferred from evidence, not from the provider's buffer flush.

## Rejected Alternatives

- ElevenLabs-owned memory as canonical memory: couples identity and retention to the voice vendor.
- Direct ElevenLabs MCP access to every JARVIS tool: violates least privilege and policy ownership.
- Caller number as authentication: vulnerable to forwarding/spoofing and insufficient for sensitive actions.
- Blind outbound-call retries: can place duplicate calls.
- Relying on provider turn-taking defaults: the values are documented to change and the turn model is proprietary, so a behaviour change would be undetectable from the JARVIS side.
- Treating the provider's `interruption` frame as a caller interruption: it is a playback clear and is emitted for non-speech input, so it would cancel replies on a cough.
- Running the SSE endpoints and the Speech Engine brain simultaneously for one number: produces two brain sessions for a single call.

## Verification Plan

- Generate/validate request fixtures from current OpenAPI without exposing generated vendor types to domain code.
- Contract-test both SSE framings, end markers, cancellation, malformed tool calls, slow first token, and disconnect.
- Contract-test the Speech Engine WS framing against a scripted brain: `conversation_initiation_client_data`, `user_audio_chunk`, `audio`, `interruption`, `ping`/`pong`, and a wrong or absent shared secret on the upgrade.
- Verify a short-lived voice-session credential cannot bind another user/workspace.
- Test MCP discovery and denied/high-risk calls with an independent ElevenLabs test agent when available.
- Mutate raw webhook bytes/signature/timestamp and replay valid events.
- Test out-of-order post-call events, no-answer/busy/unknown, and provider timeout after call acceptance.
- Place one opt-in inbound and outbound live call with recording disabled and assert correlated JARVIS/provider IDs.
- Record a real call's turn behaviour and check: a pause-heavy request is not cut off, a backchannel does not cancel a reply, and a real interruption does.
- Assert that a reply committed early (after the provider's early-speech trigger) is not surfaced as complete while more text is still arriving.

Cheapest discriminator for the central design: run a local fixture that sends an official-shape ElevenLabs Custom LLM request and rejects it unless a valid pre-created voice session maps to the expected workspace.

## Unresolved Questions

- Exact custom-server authentication headers/configuration and rotation workflow. Blocks public endpoint implementation.
- Whether the Speech Engine brain channel supports the same system-tool set as the HTTP path, and how tools are declared over the WS. Blocks Speech Engine tool parity; the HTTP path is unaffected.
- Whether a turn-boundary confidence is exposed to the brain over either transport. Blocks a measured turn-detection metric on this provider; if absent, the metric is recorded as unsupported rather than estimated.
- Whether `interruption` is emitted for backchannels and non-speech noise. Blocks correct false-interruption handling on this provider.
- Whether the selected account supports required concurrency, data residency, and outbound/SIP features. Blocks live telephony, not local voice contracts.
- Provider retry/ordering guarantees for every webhook event. Requires current webhook reference and live fixtures before reconciliation ships.
- Legal disclosure, consent, quiet-hour, and call-recording requirements by deployment jurisdiction. Blocks production outbound calling defaults.
- Selected Rust HTTP implementation versus official SDK usage. Resolve after inspecting current SDK coverage and license.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | current hosted docs, OpenAPI/AsyncAPI links | Initial architecture verification; no implementation claim | GitHub Copilot |
| 2026-09-21 | `llms.txt` index, Speech Engine + Twilio custom-LLM page, conversation-flow page | Added Speech Engine transport (WS, μ-law, signed URL, shared secret) and turn-taking settings; recorded that the turn model is proprietary and the eager-response behaviour commits replies early | GitHub Copilot |