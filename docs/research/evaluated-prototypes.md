# Research Prototypes: What Was Evaluated And Where The Findings Live

JARVIS evaluated voice and telephony options by building throwaway prototypes. Those prototypes are **not** part of the product and were deleted. This page records what each one was, what it measured, what was adopted or rejected, and where the durable evidence now lives.

Read this before concluding that a capability is unstudied. The prototypes answered their questions; the answers are in the records below.

## The Prototypes

| Prototype | What it was | Outcome | Durable record |
| --- | --- | --- | --- |
| Cascaded telephony agent | STT → LLM → TTS behind a telephony provider, text over a WebSocket | **Pattern retained** (default transport) | [twilio.md](../research/integrations/twilio.md), [elevenlabs.md](../research/integrations/elevenlabs.md) |
| Speech-to-speech agent | One realtime audio model, raw audio both directions | **Rejected as default** — ~150ms faster on one call, inside noise, in exchange for losing memory, voice selection, barge-in, and reconnection | [google-gemini-live.md](../research/integrations/google-gemini-live.md) |
| Agent-framework spike | A text-mode agent session with real tools, no room and no account | **Pattern adopted** (offline-assertable voice behaviour); adoption of the framework itself has an open boundary question | [livekit.md](../research/integrations/livekit.md) |

## Findings That Were Kept

Each of these was verified while the prototypes existed and is now part of the architecture or backlog. None of them require the prototype code to re-derive.

1. **Turn detection is a model, not a silence timer.** Twilio `speechModel=flux`, Deepgram Flux's own thresholds, ElevenLabs' turn settings, and the framework detector all converged on this. A silence timer adds the caller's pause to every turn and hides it from instrumentation. → [ADR-0010](../adr/0010-model-based-turn-detection.md)
2. **Interruption is not one behaviour.** A backchannel, a cough, and a real interruption need three different responses; a false interruption can resume playback. → [voice-and-telephony.md](../architecture/voice-and-telephony.md)
3. **A silence setting becomes a maximum cap** under model-based detection, so it can be raised without paying for it. → [twilio.md](../research/integrations/twilio.md)
4. **Two TwiML verbs that both start an audio conversation are mutually exclusive, and the platform runs only one silently.** A call can quietly run as the wrong architecture and every measurement from it is then attributed to the wrong system. → [twilio.md](../research/integrations/twilio.md)
5. **`interruptible` and `reportInputDuringAgentSpeech` are independent attributes.** Receiving caller speech does not imply playback stopped. → [twilio.md](../research/integrations/twilio.md)
6. **A telephony provider's TTS choice carries compliance eligibility**, not only quality: PCI and HIPAA eligibility depend on which STT/TTS vendor is configured. → [twilio.md](../research/integrations/twilio.md)
7. **ElevenLabs has a second custom-brain transport.** The Speech Engine path is a WebSocket brain server with `ulaw_8000` audio, a per-call signed URL, and a shared-secret upgrade — not the SSE endpoints. Choosing both against one number yields two brain sessions for one call. → [elevenlabs.md](../research/integrations/elevenlabs.md)
8. **A provider can commit a reply before its final clause exists** (early-speech trigger after a comma), so the first streamed segment is not a complete response. → [elevenlabs.md](../research/integrations/elevenlabs.md)
9. **Bundled model access is not stable.** ElevenLabs models were retired from one platform's inference bundle; using your own provider account insulates JARVIS. → [livekit.md](../research/integrations/livekit.md)
10. **A credential-free codec self-test with a waveform-correlation assertion** separates "the audio path is wrong" from "the model behaved oddly" before any paid call. → [google-gemini-live.md](../research/integrations/google-gemini-live.md)

## Findings That Were Only Leads

These appeared in prototype reasoning and are **not** verified. They must not be cited as evidence without a current official source.

- Per-minute cost arithmetic. Every figure in the prototype comparison was computed from published rates, not billed. Subscription minimums dominate at low volume and were not measured.
- Turn-detection latency for any provider. Nothing was measured on a real call; the tooling existed but the measurement did not happen.
- SIP trunk behaviour, DTMF, call transfer, and answering-machine detection on a non-Twilio platform. No account existed, so these were documented capabilities and never exercised.
- Any hosted-agent platform's end-to-end behaviour. Never tested at all.

## Rules This Established

- **Prototype code is disposable; findings are not.** A prototype that answers its question should be deleted rather than promoted, because a second implementation of the same capability inside the docs tree becomes an unowned second source of truth.
- **Prototypes do not live under `docs/`.** A prototype is a separate checkout with its own dependency tree and its own credentials, so a documentation tree never contains executable secrets or a buildable app.
- **A prototype conclusion is a lead until it is a dated record.** Anything that survives a prototype goes into `docs/research/integrations/<provider>.md` with official URLs, versions, and an access date.
- **Rejected options still get a record.** A record that says "measured, rejected, here is why" prevents the same evaluation from being repeated.
- **Never cite a prototype as authority.** After deletion it cannot be re-verified, so the record must stand alone.

## Related

- [External integration research workflow](../development/external-research.md)
- [Integration research index](../research/integrations/README.md)
- [Clean-room prototype migration](../adr/0009-clean-room-prototype-migration.md)
- [Voice and telephony architecture](../architecture/voice-and-telephony.md)
