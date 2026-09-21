---
integration: google-gemini-live
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: not selected; realtime audio model as observed in a local spike
selected_sdk: undecided
---

# Google Gemini Live (Realtime Speech-To-Speech)

## Scope

Architecture research for using a realtime, audio-in/audio-out model as a voice transport for JARVIS, and specifically for deciding whether it should be the **default** pipeline. Covers the transport shape, the audio-format problem at a telephony boundary, and the measurable claim that would justify it.

Out of scope: text chat completions, embeddings, function calling over the standard text API, image/vision features.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Google Generative Language docs index | https://ai.google.dev/llms.txt | 2026-09-21 | model and API discovery |
| Live API documentation | https://ai.google.dev/gemini-api/docs/live | 2026-09-21 | session shape, audio in/out, model naming |

This record is deliberately thin on wire detail: JARVIS has **not selected** this transport, and the decision below does not depend on its specifics. Wire-level research is required before any adapter is written.

## Verified Contract

### Operations And Transport

- A **bidirectional session** where audio is sent in and generated audio comes back, rather than a request/response pair of text turns.
- Model naming follows the vendor's versioned live-model identifiers, which change frequently; a pinned identifier must be re-checked before release.
- Audio arrives and leaves as encoded audio (PCM/base64), so the JARVIS side must handle framing and resampling rather than text tokens.

### Authentication And Authorization

- API-key authentication against the vendor endpoint. The key must live behind `SecretRef` like any other provider credential and never in a URL or log line.

### Limits And Failure Semantics

- The session is stateful and long-lived, so reconnection, resumption, and mid-session failure semantics must be characterised before it is used for real calls. This is the single largest unknown and it directly conflicts with JARVIS's "resume only at explicit persisted boundaries" rule.
- Not researched: concurrency, quota behaviour, and the maximum session duration.

### Data And Compliance

- Not researched. Realtime sessions send raw audio, which is a different disclosure surface from sending text. Retention, region, and training posture must be verified before any call carrying personal content.

### Versions And Deprecations

- Live/realtime models are explicitly in the "no grace period" category of `docs/development/external-research.md`: treat identifiers and capabilities as volatile and re-verify at implementation time.

## JARVIS Mapping

- Not adopted. If adopted, it would be a **speech-to-speech transport**, not a model gateway: it must not own memory, identity, tools, or policy, and any tool request it emits must re-enter the JARVIS tool gateway.
- Its audio would bypass the cascaded STT/TTS ports, which means the canonical transcript record would have to be produced by JARVIS from the model's output rather than received from a speech provider. That is an additional obligation, not a simplification.

## Decisions

1. **Do not adopt speech-to-speech as the default pipeline.**
2. Keep it an isolated, evaluable transport, never the only way JARVIS listens or speaks.
3. Require a measured latency requirement that a cascaded pipeline cannot meet **before** adopting it.
4. Require JARVIS to author the canonical transcript in this mode, since no speech provider produces one.

## Rejected Alternatives

- **Speech-to-speech as the default:** rejected on the evidence below.
- **Using it as the STT or TTS leg only:** rejected — it is a session-oriented realtime transport, not a drop-in STT or TTS provider, and using it that way discards its only advantage.
- **Assuming the realtime model replaces turn detection:** rejected — an end-to-end audio model still decides when the user finished, and that decision would be as invisible to JARVIS instrumentation as a silence timer, and additionally proprietary.

**Why rejection, stated as evidence rather than preference.** A side-by-side experiment in this repository (an isolated prototype, since removed) measured a raw-audio speech-to-speech call against a streamed cascaded call. The result was that the speech-to-speech path was faster by roughly **150ms perceived** — inside single-call noise — and only after the cascaded path had been given sentence-level streaming, which is what removed most of its latency in the first place. In exchange it gave up caller memory, explicit voice selection, barge-in handling, and reconnection. A capability loss that large is not paid for by a difference that cannot be distinguished from noise on one call.

## Verification Plan

- Offline: codec round-trip test asserting amplitude and waveform survive the telephony conversion, not merely that the bytes decode. A synthetic-tone self-test is the cheapest such test and needs no credentials.
- Offline: assert the transport cannot be selected without an explicit configuration flag, and that selecting it does not silently disable memory or policy.
- Live (opt-in, cost-gated): one scripted call recording perceived latency, turn behaviour on a pause-heavy utterance, and reconnection after a forced disconnect.
- Live: measure **against** a streamed cascaded baseline on the same scripted utterance, because an unpaired latency number proves nothing.

Cheapest discriminating test: the credential-free codec round-trip with a waveform-correlation assertion. It separates "the audio path is wrong" from "the model behaved oddly" before any paid call is made.

## Unresolved Questions

- Reconnection and session-resumption semantics after a mid-call failure. Blocks using it for real calls.
- Retention, region, and training posture for realtime audio. Blocks any call carrying personal content.
- Concurrency, quota, and session-duration limits. Blocks capacity planning.
- Whether a turn boundary or confidence is exposed at all. Blocks measuring turn latency, and therefore blocks any honest comparison against `flux`.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-21 | Generative Language docs index and Live API page, plus a local isolated prototype previously present in this repository | Record created to close a documented gap. The adoption decision rests on an in-repository measurement (speech-to-speech ~150ms faster, inside noise, with capability loss) rather than on vendor claims; wire detail is deliberately left unverified because the transport is not selected | GitHub Copilot |
