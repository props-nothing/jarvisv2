# ADR-0010: Turn Detection And Interruption Are Model-Based Capabilities

- Status: Accepted
- Date: 2026-09-21
- Extends: [ADR-0008](0008-provider-neutral-voice.md)

## Context

ADR-0008 made voice provider-neutral but said nothing about *turn taking*. The implicit design was the conventional one: the speech provider decides the caller has finished by waiting out a configured silence window, and JARVIS receives a final transcript.

Official documentation checked on 2026-09-21 shows every serious vendor in this space abandoned the silence window for a **model** that predicts end-of-turn from the meaning of the speech:

| Vendor | Mechanism | Verified fact |
| --- | --- | --- |
| Twilio Conversation Relay | `speechModel=flux` | Deepgram performs turn-boundary detection server-side; `speechTimeout` is forwarded as a **maximum silence duration** that forces an end-of-turn regardless of confidence |
| Deepgram Flux | dedicated `/v2/listen` model | `EndOfTurn`, `EagerEndOfTurn`, `TurnResumed` events; `eot_threshold` default `0.7`; ~260ms end-of-turn detection |
| ElevenLabs Agents | proprietary turn-taking model | `turn.turn_timeout`, `turn.turn_eagerness` (`patient`/`normal`/`eager`), `turn.soft_timeout_config` fillers |
| LiveKit Agents | on by default | audio+text detector model; also accepts Flux as STT endpointing; exposes `EotPrediction`, `OverlappingSpeech`, `AgentFalseInterruption` |

Two consequences make this an architectural decision rather than a provider setting:

1. **A silence timer's cost is invisible.** The wait happens inside the provider, *before* JARVIS sees a transcript. Any latency metric that starts at the final transcript — which is the natural implementation — cannot see it. The turn is already worse by the time measurement begins.
2. **It fails on the utterances an assistant gets most.** "Check my email and reply to the one from Sam... uh... the invoice" is cut off mid-thought by a silence timer, and the caller repeats themselves. That costs more time than the setting saved.

A second finding is that interruption is not one behaviour. A cough, a backchannel ("uh-huh", "yeah"), and a genuine interruption are different events with different correct responses. Silence-window barge-in treats them identically, so a cough stops a reply mid-sentence as if the caller had interrupted.

## Decision

**Turn detection and interruption are first-class JARVIS capabilities behind provider-neutral ports.** They are never inherited implicitly from a speech provider's defaults and never allowed to define the canonical transcript boundary alone.

1. The voice session exposes turn and interruption **events**, not just transcripts: end-of-turn with a confidence, eager end-of-turn (speculative), turn-resumed (the caller kept talking, discard the draft), and false-interruption (resume where speech stopped).
2. **Turn-detection latency is measured and reported separately** from perceived response latency. The two are additive, and folding them into one number hides the only metric that responds to this decision.
3. The silence timer survives only as a **maximum-silence cap** (a safety bound), which is what it becomes under model-based detection. It is not the turn mechanism.
4. Backchannel suppression and interruption sensitivity are explicit session policy with recorded values, because their provider defaults have changed without notice. Twilio's `reportInputDuringAgentSpeech` moved from `any` to `none` in May 2025.
5. Where a provider's turn logic is proprietary and unswappable, JARVIS still records the transcript boundary as **its own** event and keeps the provider's confidence as evidence. The provider may advise a boundary; JARVIS records it.
6. **Speech-to-speech is not the default transport.** A raw-audio realtime model was measured at roughly 150ms faster than a streamed cascaded pipeline — inside single-call noise — in exchange for losing memory, voice selection, barge-in handling, and reconnection. Cascaded with model-based turn detection is the default.

## Consequences

- Voice session contracts, storage, and audit gain turn/interruption events and an end-of-turn confidence dimension.
- A provider adapter must map its turn vocabulary onto one canonical set. A provider that cannot express false interruption reports it as unsupported rather than as a generic interruption.
- Session policy must carry turn config explicitly; relying on defaults is unsafe because those defaults are documented to change.
- The latency budget for voice is split into turn detection, context build, time to first token, and time to first audible response. Only then is a regression attributable.
- Eager end-of-turn enables speculative generation, which can increase model calls substantially (Deepgram documents 50–70%). It is off unless a measured latency problem justifies the cost.

## Alternatives

- **Silence-window turn taking (the implicit previous design):** rejected — adds an unmeasurable wait to every turn and penalises exactly the pause-heavy requests an assistant receives.
- **Delegate turn taking entirely to the provider and store nothing:** rejected — loses the ability to measure it, compare providers, or explain a bad call. The provider's decision becomes unauditable.
- **Speech-to-speech as the default:** rejected — measured gain inside noise, capability loss that is not.
- **Build a turn-detection model into JARVIS:** rejected — no evidence it would beat the models already available behind ports, and it makes JARVIS own a research problem that is not its differentiator.

## Revisit When

- A provider exposes a turn confidence with calibrated semantics, making a cross-provider threshold meaningful.
- Eager end-of-turn becomes reliable enough to default on without speculative-call cost.
- Local audio (not telephony) demands a different turn strategy, for example push-to-talk where turn detection is unnecessary.
