---
integration: livekit
status: researched
last_verified: 2026-09-21
owners: []
selected_spec_version: hosted docs as of verification date (transport index 59 pages, agents index 230 pages); agents SDK ^1.9.0 observed in a local spike
selected_sdk: undecided
---

# LiveKit (Agents, Realtime Transport, Telephony, Turn Detection)

## Scope

Architecture research for LiveKit across the surfaces JARVIS could adopt: it as a replaceable voice-agent runtime, its realtime transport (rooms, participants, tracks, data and RPC channels), its multimodal agent input, its telephony surface, its turn-detection and interruption model, and its testing model. Evaluated as an **alternative** to the ElevenLabs-hosted-agent and Twilio-relay paths, not as an addition to them.

**Researched is not adopted.** Everything below is evidence. Adoption of any surface is gated on the session-scope decision (`P8-017`) and the placement ADR (`P8-016`), and nothing here is implemented.

Narrowed rather than excluded — each is a JARVIS scope decision with a named owner, not a research gap:

- **Video and screen share**: in research scope. Whether a JARVIS session may carry them is decided by `FR-VOICE-007` and `P8-017`.
- **Multi-participant rooms**: in research scope. The default stays one authenticated caller; extra participants require recorded session scope.
- **Realtime data and RPC channels**: in research scope as transport only. Any tool effect they trigger re-enters the JARVIS tool gateway under ADR-0005.
- **Frontend UI components**: out of scope. The desktop and web client is Tauri plus React/TypeScript (`ui/web`); LiveKit frontend libraries are not adopted.
- **Robotics and Portal**: out of scope; no JARVIS requirement.
- **Self-hosting the SFU**: out of scope for v1. Distributed self-hosting requires Redis, which is in `ROADMAP.md` "Explicitly Deferred"; `P10-007` requires a measured bottleneck and an ADR before adopting optional infrastructure.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| Docs index | https://docs.livekit.io/llms.txt | 2026-09-21 | section discovery; agent instructions (append `.md`, section `llms.txt`, docs MCP server) |
| Telephony index | https://docs.livekit.io/telephony/llms.txt | 2026-09-21 | 41 pages: trunks, dispatch rules, features, providers, APIs |
| Telephony overview | https://docs.livekit.io/telephony.md | 2026-09-21 | SIP integration model |
| Accepting / making calls | https://docs.livekit.io/telephony/accepting-calls.md, https://docs.livekit.io/telephony/making-calls.md | 2026-09-21 | inbound trunk + dispatch rule, outbound trunk, outbound calls |
| Provider quickstarts | https://docs.livekit.io/telephony/start/providers/twilio.md and sibling pages | 2026-09-21 | Twilio, Telnyx, Plivo, Wavix, Sinch, didlogic |
| Features | https://docs.livekit.io/telephony/features.md | 2026-09-21 | DTMF, answering-machine detection, region pinning, HD voice, secure trunking |
| Transfers | https://docs.livekit.io/telephony/features/transfers.md | 2026-09-21 | cold (SIP REFER) and warm (agent-assisted) transfer |
| Connectors | https://docs.livekit.io/telephony/connectors.md | 2026-09-21 | Twilio (WebSocket) and WhatsApp connectors |
| Agents index (turn detection, testing, tools) | https://docs.livekit.io/agents/llms.txt | 2026-09-21 | turn detection and interruptions, tool calling, testing/simulations |
| Transport section index | https://docs.livekit.io/transport/llms.txt | 2026-09-21 | 59 pages: media (camera & microphone, screen share, raw tracks, frame metadata, noise & echo cancellation), data (text streams, byte streams, RPC, data tracks, data packets), state sync (participant attributes, room metadata), end-to-end encryption, Egress/Ingress, self-hosting |
| Agents section index | https://docs.livekit.io/agents/llms.txt | 2026-09-21 | 230 pages: multimodality (speech & audio, images & video), logic (sessions, tools, MCP, RPC forwarding to frontend), turn detection & interruptions (turn detector, adaptive interruption handling, Silero VAD, tuning incl. preemptive generation), testing/simulations, agent server, models |
| Rust SDK repository | https://github.com/livekit/rust-sdks | 2026-09-21 | official Rust realtime and server SDK; crate list, feature list, build requirements, Apache-2.0 |
| SDK matrix | https://docs.livekit.io/reference.md#livekit-sdks | 2026-09-21 | client SDKs include Rust; Server APIs include Rust; **Agents SDKs are Python and Node.js only** |

Section indexes were read in full. Individual page **bodies** for the vision, RPC, and turn-tuning pages were not opened in this pass; fetch them at implementation time before relying on wire or API detail.

## Verified Contract

### What It Is

An open-source WebRTC SFU plus an **agent framework** (Python and Node.js). Every participant — browser, agent, phone call via SIP — joins a **room**, so agent code is the same regardless of how the human connected. Agents are dispatched into rooms by an agent server. Local spike observed `@livekit/agents` `^1.9.0` with `@livekit/local-inference` `^0.2.7`.

### Realtime Transport, Data, And Media

Beyond audio, the transport section documents a full realtime surface. Recorded as capability, not as adopted scope:

- **Media**: publishing camera and microphone, screen sharing, subscribing to tracks, processing raw tracks, pre-encoded video, per-frame metadata, codecs, noise and echo cancellation.
- **Data**: text streams, byte streams (files/images), **remote procedure calls**, data tracks, low-level data packets.
- **State synchronization**: per-participant attributes and room metadata.
- **End-to-end encryption**: E2EE overview, get-started, and E2EE with agents.
- **Egress / Ingress**: record or livestream a room out, and ingest non-WebRTC streams in.
- **Self-hosting**: local, VM, Kubernetes, distributed multi-region, firewall/ports, benchmarks, and the SIP server. **Distributed mode requires Redis** as shared data store and message bus; Egress and Ingress also use Redis messaging queues.

The Agents framework additionally documents **image and video input** for agents (vision: images, video frame sampling, live video input, virtual avatars), **wakeword detection** on the client, tool **forwarding to the frontend via RPC**, **MCP** server tools, and a documented **LangGraph/LangChain** LLM integration.

### Rust SDK: The Finding Is The Build Cost

`livekit/rust-sdks` publishes `livekit` (realtime), `livekit-api` (server APIs and token generation), `livekit-protocol`, `livekit-rpc`, `livekit-data-stream`, `livekit-datatrack`, and `livekit-wakeword`. Licensing is **Apache-2.0**, which is already in `deny.toml`'s allow list.

Feature list includes receive/publish tracks, data channels, simulcast, SVC codecs (AV1/VP9), adaptive streaming, Dynacast, and hardware video encode/decode.

Adoption cost is the finding, not the API:

- Tokio is required.
- The repository instructs consumers to add its `rustflags` from `.cargo/config.toml`, "otherwise linking may fail".
- `webrtc-sys` compiles against a hermetic libc++ that tracks LLVM trunk and requires **clang 21 or later**; the documented Linux build also installs `libglib2.0-dev`, `libclang-dev`, `libjpeg-turbo8-dev`, and CUDA toolkit for NVENC.
- macOS builds need the `-ObjC` linker flag or the app aborts with an unrecognized selector.
- Supported platform toolkits are enumerated in `builds.yml`.

This is a **native C/C++ build dependency**, not a pure-Rust crate. It interacts directly with the zero-toolchain install requirement (`FR-INSTALL-002`) and with the existing constraint that bundled SQLite cannot be cross-compiled here.

### Telephony (documented, not exercised)

| Capability | Documented page |
| --- | --- |
| Inbound trunk | `telephony/accepting-calls/inbound-trunk` |
| Dispatch rule | `telephony/accepting-calls/dispatch-rule` |
| Twilio Voice integration via TwiML/conferencing | `telephony/accepting-calls/inbound-twilio` |
| Outbound trunk and outbound calls | `telephony/making-calls/outbound-trunk`, `.../outbound-calls` |
| DTMF | `telephony/features/dtmf` |
| Answering-machine detection | `telephony/features/answering-machine-detection` |
| Cold transfer via SIP REFER | `telephony/features/transfers/cold` |
| Warm agent-assisted transfer with context handoff | `telephony/features/transfers/warm` |
| Region pinning | `telephony/features/region-pinning` |
| Secure trunking, HD voice | `telephony/features/secure-trunking`, `.../hd-voice` |
| Codec negotiation | `reference/telephony/codecs-negotiation` |
| Phone Number API, SIP API | `reference/telephony/phone-numbers-api`, `reference/telephony/sip-api` |
| WhatsApp and Twilio connectors | `telephony/connectors/whatsapp`, `telephony/connectors/twilio` |
| Testing a telephony setup | `telephony/testing` |

Providers with documented SIP setup: Twilio, Telnyx, Plivo, Wavix, Sinch, didlogic. LiveKit can also provision numbers directly.

### Turn Detection And Interruption

- A **turn detector model runs by default** — described as predicting end-of-turn from the meaning of speech (audio + text) on top of VAD, rather than a silence timer. No configuration is required to get it.
- Flux is also supported as **STT endpointing** (`turnDetection: 'stt'`), alongside AssemblyAI — independent corroboration that the ecosystem converged on model-based turn detection.
- Turn-taking surfaces as events including `user_state_changed`/`agent_state_changed`, `EotPrediction`, `OverlappingSpeech`, and `AgentFalseInterruption`, with `resumeFalseInterruption` to continue where speech stopped.
- `AgentFalseInterruption` — "the agent stopped because it heard speech that turned out to be nothing" — has no equivalent in the current JARVIS design, where any detected speech cancels a reply.
- The audio turn detector is a **local inference model** (`@livekit/local-inference`), so first use may download weights.

### Testing

- Documented text-mode testing: the agent runs in a text session **with no room connection**, so tool wiring is assertable in CI without an account, a key, or a phone call.
- Local spike evidence (this repository, isolated folder, since removed — see below): 4 headless tests passed with no account; a real model chose a tool over the OpenAI-compatible chat path, confirmed by two independent signals (the framework's own executed-tools event and a recorder inside the tool).
- Documented "simulations" and agent-testing pages extend this beyond unit-level assertions.

### Limits And Failure Semantics

- The agent server handles dispatch, load balancing, and graceful shutdown.
- Self-hosting the SFU avoids transport cost but moves STT/LLM/TTS onto your own provider accounts; the enhanced noise-cancellation used in their examples is a platform-only feature.

### Data And Compliance

- Deployment to the hosted platform is a managed service; self-hosting changes the data path. Retention/PII behaviour differs between the two and must be recorded per deployment.
- Docs list observability features including **PII redaction** and data hooks, which are relevant to the JARVIS requirement that traces never contain raw transcripts by default.

### Versions And Deprecations

- Platform risk is documented fact, not speculation: ElevenLabs models were retired from LiveKit Inference on 2026-08-31. The plugin still works with your own ElevenLabs account. A bundled-model dependency can be removed from a bundle, so using your own provider account insulates JARVIS.

## JARVIS Mapping

- The transport surface is **broader than the JARVIS voice session contract**. A room is a multi-participant, multi-track container with its own state store; the current contract models one authenticated caller and a linked run. Adopting rooms would make "session" a room-scoped concept and require a scope decision (see `P8-017`) before any transport code.
- **RPC and data tracks are a second, framework-owned path to an effect.** ADR-0005 makes JARVIS the canonical tool gateway. If adopted, a LiveKit RPC is transport only; any tool effect it triggers must still enter the JARVIS policy pipeline, exactly as a voice brain tool call does.
- **End-to-end encryption puts media outside JARVIS inspection.** That is a policy decision interacting with recording, transcript, and retention rules, not an implementation detail. It is not adopted by default.
- Distributed self-hosting requires **Redis**, which is in `ROADMAP.md` "Explicitly Deferred". Multi-region SFU is out of v1 scope; `P10-007` requires a measured bottleneck and a new ADR before optional infrastructure.
- Agent **avatars** are output rendering. They carry no identity and no canonical state.
- The Rust SDK's native build toolchain is evaluated for the **native installer lane** separately from feature fit; a feature is not adoptable if it breaks `FR-INSTALL-002`.

**This is the open architectural question, and it must be answered before any implementation.**

- A LiveKit agent is a Node/Python program. It is therefore either a **runtime worker** under the existing runtime supervisor, or an adapter inside `jarvis-voice`. The two have different obligations:
  - **If a runtime:** its tool requests must re-enter the JARVIS tool gateway (`P7-003`) and unmediated native tools stay disabled. It must not hold canonical memory, and its lifecycle is supervised like any other runtime worker.
  - **If an adapter:** it must not own memory, identity, or policy, and its session must map onto the JARVIS voice-session contract.
- Either way: LiveKit must never become the platform. JARVIS retains identity, memory, policy, approvals, audit, and the turn record ([ADR-0006](../../adr/0006-isolated-agent-runtimes.md), [ADR-0008](../../adr/0008-provider-neutral-voice.md)).
- Its turn and interruption events map onto the canonical turn vocabulary in [ADR-0010](../../adr/0010-model-based-turn-detection.md); `AgentFalseInterruption` is the reason that vocabulary includes a false-interruption event.
- Twilio remains usable as the SIP carrier, so adopting this does not displace the existing telephony record.
- A local-inference turn detector downloading weights on first use is a first-run latency and offline-install concern.

## Decisions

1. Treat LiveKit as a **replaceable runtime/adapter candidate**, not a platform, and require an ADR fixing its boundary before code.
2. Do not adopt its telephony until an account-level test places one real inbound and one real outbound call. Documentation is not evidence of a working trunk.
3. Adopt the **turn-detection event vocabulary** regardless of whether LiveKit is adopted, because it is provider-neutral and identifies a real gap (false interruption).
4. Use its text-mode testing idea as a pattern for JARVIS voice testing: agent behaviour should be assertable without a phone call, which also satisfies `NFR-TEST-001`.
5. Prefer own provider accounts over bundled inference, evidenced by the 2026-08-31 ElevenLabs retirement.
6. Record the realtime transport, data/RPC, E2EE, and Rust SDK surfaces as **evidence only**. No adoption without the session-scope decision (`P8-017`) and the boundary ADR (`P8-016`).
7. Treat the Rust SDK's native toolchain requirement as a **first-class adoption risk**, not a footnote, because it conflicts with the zero-toolchain install requirement.

## Rejected Alternatives

- **LiveKit as the trusted control plane:** rejected — contradicts ADR-0001/0002/0006; it is a runtime, not an authority.
- **Replacing Twilio rather than using it as a carrier:** rejected — no requirement it satisfies better, and it discards a verified telephony path.
- **Adopting it on latency grounds alone:** rejected — the measured per-call difference against a streamed cascaded pipeline was inside single-call noise.
- **Assuming its bundled model catalogue is stable:** contradicted by the documented retirement.

## Verification Plan

- Offline: run the framework's text-mode session with no room and assert a tool is called by name with controlled arguments.
- Offline: assert a tool request originating from the runtime re-enters JARVIS policy and cannot bypass approval.
- Offline: map each framework turn/interruption event onto the canonical vocabulary and fail on any unmapped event rather than dropping it.
- Live (opt-in, account required): one inbound and one outbound call through a Twilio-trunked number, asserting DTMF, transfer, and answering-machine detection behave as documented.
- Live: measure turn-detection latency separately from response latency, and record first-run cost of downloading local-inference weights.

Cheapest discriminating test: a text-mode session that asserts a tool call without any room, account, or phone call. If that cannot be made to pass offline, the headline benefit does not exist.

## Unresolved Questions

- **Where the boundary sits** (runtime worker vs `jarvis-voice` adapter). Blocks all implementation.
- Whether its turn detector can be swapped or configured off, and what its licence/runtime cost is for a self-hosted install. Blocks shipping it inside a native install.
- Telephony behaviour for SIP trunks, DTMF, transfer, and answering-machine detection under a real account. Blocks replacing the current telephony path.
- Cost of self-hosted operation versus hosted, including model keys. Blocks a cost comparison.
- Whether agent state can be reconstructed from JARVIS canonical state after a crash, since the room is not canonical.
- Whether JARVIS adopts a room as a session concept at all, or keeps one caller ↔ one run and treats multi-participant as a separate capability. Blocks every transport decision.
- Whether E2EE can coexist with recording, transcript, and audit policy, or must be excluded by policy. Blocks any transport adoption where inspection is required.
- Whether the Rust `livekit` crate can be shipped in the native installer lane given its clang-21/libc++/rustflags build requirements. Blocks using the Rust SDK rather than an out-of-process worker.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-21 | LiveKit docs index and telephony index (41 pages), agents index, plus a local headless spike at `@livekit/agents` ^1.9.0 | Initial record. Telephony is fully documented (trunks, dispatch, DTMF, transfers, AMD, regions, six provider quickstarts, connectors); turn detector is on by default with a false-interruption event; text-mode testing is documented. No account held, so no call was placed | GitHub Copilot |
| 2026-09-21 | docs index, transport index (59 pages), agents index (230 pages), `reference.md` SDK matrix, `livekit/rust-sdks` README | Second pass. Transport, data/RPC, E2EE, Egress/Ingress, and self-hosting recorded; multimodal vision, wakeword, MCP, and LangGraph integrations recorded; Rust client and server SDKs confirmed Apache-2.0 but confirmed to require a native C/C++ toolchain (clang 21+, rustflags, `-ObjC`); distributed self-hosting confirmed to require Redis. No page bodies opened for vision/RPC/turn-tuning. Still no account, so no call placed | GitHub Copilot |
