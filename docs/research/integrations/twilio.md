---
integration: twilio
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: current Programmable Voice / Conversation Relay as of verification date
selected_sdk: undecided
---

# Twilio (Programmable Voice, Conversation Relay, SIP)

## Scope

Architecture research for inbound and outbound calls, the two mutually exclusive audio-conversation paths (`<ConversationRelay>` and `<Connect><Stream>`), conversation-relay attributes that affect turn taking, SIP trunk usage as a carrier, and answer/handoff signalling.

Out of scope: SMS, Verify, Flex, Conversation Intelligence, Conversation Orchestrator.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| TwiML `<ConversationRelay>` reference | https://www.twilio.com/docs/voice/twiml/connect/conversationrelay | 2026-09-21 | full attribute set, defaults, mutual exclusivity, action callback |
| Conversation Relay messages | https://www.twilio.com/docs/voice/conversationrelay/websocket-messages | 2026-09-21 | prompt/DTMF/text/end-session framing |
| Media Streams | https://www.twilio.com/docs/voice/media-streams | 2026-09-21 | raw audio path, `ulaw_8000` |
| Twilio Voice pricing (US) | https://www.twilio.com/en-us/voice/pricing/us | 2026-09-21 | per-minute rates used in cost planning |
| Twilio PCI responsibility matrix | https://www.twilio.com/en-us/pci-compliance | 2026-09-21 | provider-level compliance differences |

No usable single `llms.txt` at the docs host on this date; individual reference pages were used, and the page's own `dateModified` was recorded where present.

## Verified Contract

### Operations And Transport

Two **mutually exclusive** ways to give a call a conversational agent, both under the `<Connect>` verb:

| | `<ConversationRelay>` | `<Connect><Stream>` |
| --- | --- | --- |
| Who does STT/TTS | Twilio | JARVIS |
| What JARVIS exchanges | JSON text over a WebSocket | raw base64 audio |
| Audio format | Twilio-managed | `audio/x-mulaw`, 8000 Hz |
| Cost shape | AI bundle rate | raw media rate |

- The `url` attribute is required and **must begin with `wss://`**.
- A number routes **either** to a trunk **or** to a webhook, not both. A number attached to an Elastic SIP Trunk must be detached before a webhook can receive calls.
- `<Connect action="...">` receives the session result: `SessionId`, `SessionStatus` (`completed` / `ended` / `failed`), `SessionDuration`, and on failure `ErrorCode`/`ErrorMessage`. Application-initiated ends carry `HandoffData`.
- Custom `<Parameter>` values arrive in the initial WebSocket `setup` message under `customParameters`.
- Language can be set per `<Language>` child, and changed mid-call.

### Turn-Taking And Interruption Attributes (verified 2026-09-21, page `dateModified` 2026-06-22)

| Attribute | Values | Default | Effect |
| --- | --- | --- | --- |
| `speechModel` | provider-dependent; **`flux` enables model-based turn detection** | `nova-3-general`, else `nova-2-general`; `telephony` for Google | selects STT model; `flux` moves turn boundary detection server-side |
| `eotThreshold` | 0.5–0.9 | 0.8 | confidence required to finish a turn; **only applies with Deepgram + `flux`** |
| `partialPrompts` | `true`/`false` | `false` | send unfinalised prompts and **eager** end-of-turn events with `last=false`; Deepgram + `flux` only |
| `speechTimeout` | integer 600–5000 ms | `auto` | silence wait before the final prompt — **under `flux` this is forwarded as a maximum silence duration** that forces end-of-turn regardless of confidence |
| `interruptible` | `none`/`dtmf`/`speech`/`any` (booleans accepted) | `any` | what caller input stops TTS playback |
| `reportInputDuringAgentSpeech` | `none`/`dtmf`/`speech`/`any` | **`none`** (was `any` before May 2025) | whether JARVIS *receives* input while the agent speaks, independent of interruption |
| `interruptSensitivity` | `high`/`medium`/`low` | `high` | how easily speech interrupts playback |
| `ignoreBackchannel` | `true`/`false` | `false` | filter short acknowledgements so they neither interrupt nor trigger a turn |
| `welcomeGreetingInterruptible` | `none`/`dtmf`/`speech`/`any` | `any` | interruptions during the greeting |
| `preemptible` | `true`/`false` | `false` | whether the next talk cycle's tokens can interrupt the current TTS |
| `dtmfDetection` | `true` | — | emit DTMF events over the WebSocket |
| `events` | `speaker-events`, `tokens-played` | — | `tokens-played` is the only proof words reached the caller |
| `elevenlabsTextNormalization` | `on`/`auto`/`off` | `off` | text normalisation for the ElevenLabs TTS provider; `auto` behaves as `off` for relay calls |
| `transcriptionProvider` | `Google`, `Deepgram` | `Deepgram` (accounts older than 2025-09-12 may default to Google) | STT provider |
| `ttsProvider` | `Google`, `Amazon`, `ElevenLabs` | `ElevenLabs` | TTS provider |
| `hints` | comma-separated phrases | — | bias recognition toward expected names/terms |
| `deepgramSmartFormat` | `true`/`false` | `true` | reformat dates, times, currency, numbers, addresses |

Two consequences JARVIS should not discover at runtime:

- **`interruptible` and `reportInputDuringAgentSpeech` are independent.** With `interruptible=none` and `reportInputDuringAgentSpeech=speech`, caller speech reaches JARVIS **without** stopping playback. A design that assumes "we received speech" implies "playback stopped" is wrong.
- **An unknown attribute name is silently dropped by the SDK**, so a typo looks applied and does nothing. Attribute presence must be asserted on the serialised TwiML.

### Automatic Language Detection

`multi` mode requires `transcriptionProvider=Deepgram` **and** `ttsProvider=ElevenLabs`; any other combination errors and **ends the session**. With `multi`, `lang` carries only the primary BCP-47 tag (`en`, not `en-US`).

### Authentication And Authorization

- Inbound webhooks are verified with `X-Twilio-Signature` HMAC over the URL plus form parameters using the account auth token. Without validation, anyone reaching the URL can originate billable calls.
- Custom `customParameters` and `HandoffData` are **not** identity evidence and are not treated as PCI data by Twilio, so no PCI data may be placed in them.

### Limits And Failure Semantics

- `speechTimeout` bounds are enforced (600–5000 inclusive).
- Relay sessions report termination through the `action` callback; a network failure to the WebSocket server surfaces as a session failure with an error code.
- Provider acceptance does not mean a person answered.

### Data And Compliance

- Twilio publishes per-model AI nutrition facts for the STT/TTS vendors it routes through, including whether customer data trains the base model.
- **PCI and HIPAA eligibility depend on provider configuration.** The docs state not all TTS/transcription providers are guaranteed PCI compliant, and that relay is HIPAA-eligible only when configured properly with a signed BAA. Provider choice is therefore a compliance decision, not only a quality decision.
- TwiML attributes such as `welcomeGreeting`, `hints`, `customParameters`, and `HandoffData` are explicitly **not** treated as containing PCI data — never place such data there.

### Versions And Deprecations

- Default changes are documented: `reportInputDuringAgentSpeech` moved `any` → `none`; the default transcription provider depends on account age. Both are reasons to set values explicitly.

## JARVIS Mapping

- Telephony is a **carrier/transport** adapter. It never owns identity, memory, policy, or the turn contract.
- `voice.call.start/status/end` map to outbound APIs; inbound calls arrive as verified webhooks.
- Turn attributes map to the voice-session turn policy; the **effective** values are persisted as evidence next to the requested ones.
- Relay `SessionId`/`CallSid` are provider correlation evidence and never replace JARVIS call IDs.
- Engine selection (`ConversationRelay` vs raw Media Streams) is a single explicit decision per call, recorded on the call record.
- Twilio remains usable as a SIP carrier even when another platform owns the agent runtime, so it is not displaced by a runtime choice.

## Decisions

1. Default to `<ConversationRelay>` with `speechModel=flux`; it moves turn detection server-side and removes an unmeasurable wait from every turn.
2. Set `interruptSensitivity`, `interruptible`, `reportInputDuringAgentSpeech`, and `ignoreBackchannel` explicitly and persist them.
3. Assert attribute presence in a serialised-TwiML test, because the SDK drops unknown names silently.
4. Keep the raw-audio path available but not default; it costs JARVIS the STT/TTS/turn bundle.
5. Select the engine in exactly one place and log it, because running both verbs fails silently.
6. Validate `X-Twilio-Signature` before parsing anything.
7. Do not put PCI data in TwiML attributes, `customParameters`, or `HandoffData`.

## Rejected Alternatives

- **Silence-window turn taking (`speechModel` left at a plain transcription model):** adds the caller's pause to every turn and hides it from JARVIS instrumentation.
- **A Boolean reading of `interruptible` as "is speech reported":** conflates two independent attributes.
- **Deriving caller identity from `customParameters`:** provider-supplied and unauthenticated.
- **Assuming `ttsProvider` choice is quality-only:** it also carries PCI/HIPAA eligibility.

## Verification Plan

- Offline: assert required attributes appear in serialised TwiML for both verbs, including `speechModel=flux` and each interruption attribute.
- Offline: contract-test the relay WebSocket message framing against recorded sanitised fixtures, including `setup`/`customParameters`, prompt `last`, DTMF, and text tokens.
- Offline: reject a request whose `X-Twilio-Signature` is absent, altered, or replayed.
- Offline: prove that both verbs present at once is rejected by the builder rather than silently resolved.
- Live (opt-in, cost-gated): one inbound and one outbound call with recording disabled; assert correlated IDs and that `SessionStatus` reconciles.
- Live: compare `speechModel=flux` against a silence-based model on the same scripted pause-heavy utterance and record turn-detection latency separately.

Cheapest discriminating test: a serialised-TwiML assertion that `flux` and the interruption attributes are present, because a wrong or misspelled attribute is silently accepted and silently ineffective.

## Unresolved Questions

- Whether `partialPrompts` (eager end-of-turn) is worth its speculative model cost on JARVIS traffic. Blocks enabling eager turn handling; not a blocker for basic turn detection.
- Exact provider-level PCI/HIPAA eligibility for each STT/TTS combination under JARVIS's intended configuration. Blocks any deployment handling regulated data.
- Per-region rate/concurrency limits for relay sessions. Blocks capacity planning, not correctness.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-21 | Conversation Relay reference (`dateModified` 2026-06-22), Media Streams, voice pricing | Initial record. Captured the full attribute set with defaults, the `flux` semantics of `speechTimeout`, the independence of `interruptible`/`reportInputDuringAgentSpeech`, the silent-drop behaviour for unknown attribute names, `multi`-mode provider constraints, and the PCI/HIPAA provider dependency | GitHub Copilot |
