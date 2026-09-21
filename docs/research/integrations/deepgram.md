---
integration: deepgram
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: Flux on /v2/listen as of verification date
selected_sdk: undecided
---

# Deepgram (Flux Turn Detection, Nova STT)

## Scope

Architecture research for streaming speech-to-text and, specifically, **model-integrated end-of-turn detection** used either directly or indirectly through Twilio Conversation Relay. Covers the connection contract, the turn-event vocabulary and its thresholds, and the cost implication of eager turn handling.

Out of scope: batch/async transcription, keyterm prompting, entity detection, TTS voices.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Flux quickstart | https://developers.deepgram.com/docs/flux/quickstart | 2026-09-21 | endpoint, model names, audio requirements, chunk size, event names, parameters, latency claim |
| Flux end-of-turn configuration | https://developers.deepgram.com/docs/flux/configuration | 2026-09-21 | threshold semantics and ranges |
| Flux state machine | https://developers.deepgram.com/docs/flux/state | 2026-09-21 | turn lifecycle |
| Flux eager end-of-turn | https://developers.deepgram.com/docs/flux/voice-agent-eager-eot | 2026-09-21 | speculative generation and its cost |
| Flux force end turn | https://developers.deepgram.com/docs/flux/force-end-turn | 2026-09-21 | suppressing natural end-of-turn (`eot_threshold=1.0`) |
| Flux configure control message | https://developers.deepgram.com/docs/flux/configure | 2026-09-21 | mid-stream parameter updates |

## Verified Contract

### Operations And Transport

- Flux uses a **separate endpoint**: `/v2/listen`. `/v1/listen` does not work with Flux.
- Model selection is part of the model name: `flux-general-en` (English) or `flux-general-multi` (multilingual). There is no `model=flux`.
- Direct WebSocket: `wss://api.deepgram.com/v2/listen?model=flux-general-en&...`, with the API key sent as an `Authorization: Token <key>` header.
- SDKs expose it as `client.listen.v2.connect(...)` (Python/JS) or `deepgram.listen().v2().v2WebSocket()` (Java). .NET requires SDK v6.9.0+; the Go SDK is not yet available.
- **Audio format requirements (raw):** `encoding` **and** `sample_rate` are both **required**; supported rates are 8000, 16000, 24000, 44100, 48000 with 16000 recommended. Containerised input (WAV/Ogg/WebM) requires these to be **omitted** so they can be auto-detected. Supported raw encodings include `mulaw`, `alaw`, `linear16`, `linear32`, `opus`.
- **Chunk size: ~80ms is strongly recommended** for latency and model performance. The docs' own examples use 2560 bytes at 16kHz linear16.
- `language_hint` is supported only by `flux-general-multi`. Passing it to `-en`, or using a `language=` parameter, are documented mistakes.
- Parameters can be updated **mid-stream** through a configure control message, without reconnecting.

### Turn Events And Configuration

Event vocabulary observed in the docs: `Connected`, `TurnInfo` (carrying transcript, words, languages), `EndOfTurn`, `EagerEndOfTurn`, `TurnResumed`, `Error`, plus a stop/close stream.

| Parameter | Range | Default | Meaning |
| --- | --- | --- | --- |
| `eot_threshold` | 0.5–1.0 | **0.7** | confidence required to fire `EndOfTurn`; higher is more reliable but slightly slower; `1.0` suppresses natural end-of-turn entirely and requires driving turns with an explicit force-end-turn |
| `eager_eot_threshold` | 0.3–0.9 | none | required to enable `EagerEndOfTurn`; lower fires earlier with more false starts |
| `eot_timeout_ms` | 500–60000 | **5000** | maximum silence before forcing an `EndOfTurn` **regardless of confidence** |

`eager_eot_threshold` also enables `TurnResumed`, which signals that the caller continued speaking and a draft response should be cancelled.

Documented cost: enabling `EagerEndOfTurn` can increase LLM API calls by **50–70%** due to speculative generation.

Documented latency claim: **~260ms end-of-turn detection**.

### Authentication And Authorization

- `Authorization: Token <api_key>` header on the WebSocket upgrade (not a query parameter in the documented form).
- SDKs read `DEEPGRAM_API_KEY` from the environment.

### Limits And Failure Semantics

- Parameter ranges are enforced; `eager_eot_threshold` is *not* defaulted, so eager events do not occur unless explicitly enabled.
- `eot_timeout_ms` is a safety bound rather than the primary mechanism, and it fires "regardless of confidence" — meaning a very long `eot_timeout_ms` trades away the responsiveness this model exists for.

### Data And Compliance

- Twilio's AI nutrition facts for the Deepgram STT route state customer data is not used to train the base model and is not stored or retained in it. This is Twilio's description of its own route; a direct Deepgram account has its own terms and must be verified separately before relying on it.

### Versions And Deprecations

- Flux is presented as the current turn-detection model with Nova-3-level accuracy. Nova models remain the non-turn-detecting option.

## JARVIS Mapping

- Flux supplies the **turn boundary**; it does not own the transcript record or the turn policy. JARVIS records the boundary event and its confidence as evidence.
- `EndOfTurn` → canonical turn boundary. `EagerEndOfTurn` → permission to start speculative work. `TurnResumed` → discard that work. All three are canonical session events.
- `eot_threshold`, `eager_eot_threshold`, and `eot_timeout_ms` are voice-session policy, persisted with the requested and effective values.
- The 80ms chunk guidance and the 16kHz recommendation are adapter transport concerns and belong in the codec/resampling layer, covered by a round-trip test.
- `mulaw` support at 8000 Hz means a telephony path can avoid resampling entirely for STT if the rest of the pipeline allows it.

## Decisions

1. Use Flux when model-based turn detection is required, selected by model name and the `/v2/listen` endpoint.
2. Leave eager turn handling **off by default**; enable only against a measured latency problem, because it multiplies model calls.
3. Keep `eot_timeout_ms` as a bounded safety cap, not the turn mechanism, and record its value.
4. Send raw audio with explicit `encoding`/`sample_rate`; never rely on defaults where the docs mark them required.
5. Treat `/v2/listen` as a distinct adapter path, since `/v1/listen` silently is not Flux.

## Rejected Alternatives

- **`eot_threshold=1.0` as a generic setting:** it suppresses natural end-of-turn and makes JARVIS responsible for deciding every turn. Rejected except for a deliberate push-to-talk or forced-turn design.
- **Enabling eager end-of-turn by default:** rejected — 50–70% more model calls for a latency gain that has not been measured on JARVIS traffic.
- **Using `/v1/listen` with a Flux model name:** rejected — documented to not work.
- **Inferring turns from transcript timestamps:** that is the silence-window approach this model replaces.

## Verification Plan

- Offline: contract-test the adapter against recorded sanitised Flux frames covering `Connected`, `TurnInfo`, `EndOfTurn`, `EagerEndOfTurn`, `TurnResumed`, and `Error`.
- Offline: assert the adapter refuses to construct a Flux session against `/v1/listen`, and that it sends `encoding`/`sample_rate`.
- Offline: assert eager handling is disabled unless a confidence threshold is configured.
- Offline: audio round-trip test asserting amplitude and waveform survive resampling, not merely that it decodes.
- Live (opt-in): one scripted pause-heavy utterance, recording turn-detection latency separately from response latency, and one backchannel/noise case to confirm it does not fire a turn.

Cheapest discriminating test: feed a scripted long-pause utterance and assert `EndOfTurn` is not emitted at the earliest silence, only at the pause the model judges to be an actual turn boundary. A silence-based implementation passes every framing test and fails only this one.

## Unresolved Questions

- Deepgram's own (non-Twilio-routed) retention and training terms for a direct account. Blocks a direct integration handling sensitive data.
- Whether `TurnResumed` is emitted for backchannels as well as genuine continuation. Blocks correct false-interruption handling.
- Concurrency and per-second rate limits for concurrent voice sessions. Blocks capacity planning.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-21 | Flux quickstart and configuration pages | Initial record. Captured `/v2/listen` requirement, model naming, mandatory raw `encoding`/`sample_rate`, 80ms chunk guidance, the three turn parameters with defaults, the `TurnResumed` cancellation signal, and the 50–70% cost of eager handling | GitHub Copilot |
