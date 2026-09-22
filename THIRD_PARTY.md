# Third-Party Research And Provenance

This project studies public architecture and documentation but does not import third-party source code by default. Before copying or adapting code, verify the exact file's license, notices, compatibility with the intended JARVIS license, and attribution requirements.

## Local Prototype

The [example](example/readme.md) identifies itself as **Creative Commons BY-NC 4.0** and credits FatihMakes. Treat it as a quarantined behavior reference. Do not copy its implementation into a commercial or differently licensed JARVIS distribution without explicit legal review or permission.

Behavioral ideas captured from the prototype include:

- interface-issued confirmation for irreversible actions
- undo for reversible local changes
- self-describing action discovery
- local wake-word gating and push-to-talk
- echo suppression and lip-sync timing
- source-labelled screen capture and multi-monitor coordinate checks
- on-demand memory recall instead of dumping all memory into prompts
- honest distinction between submitted and confirmed external effects

The clean-room migration record is [docs/migration/python-prototype.md](docs/migration/python-prototype.md).

## Upstream Architecture Study

Reviewed on **2026-09-20**:

| Project | Official source | Pattern studied |
| --- | --- | --- |
| OpenClaw | https://github.com/openclaw/openclaw and https://docs.openclaw.ai/llms.txt | Gateway/client topology, runtime and plugin ownership, doctor/repair, service lifecycle, scoped tools, updates |
| Goose | https://github.com/aaif-goose/goose | Rust workspace, CLI/desktop/API surfaces, MCP extensions, ACP, native release artifacts |
| Tailscale | https://github.com/tailscale/tailscale | Long-lived daemon plus CLI, local API, OS-specific services, state paths, update and integration tests |
| Tauri | https://github.com/tauri-apps/tauri and https://v2.tauri.app/llms.txt | Native desktop packaging, Rust/webview boundaries, updater and platform bundles |
| MCP | https://github.com/modelcontextprotocol/rust-sdk and https://modelcontextprotocol.io/llms.txt | Client/server separation, stdio, Streamable HTTP, protocol negotiation, OAuth, conformance |

`P3-007` selected **`rmcp` 3.4.0** for MCP (`Apache-2.0`, Rust SDK Tier 1) against protocol revision **`2026-07-28`**. It is not yet a dependency; the licence and MSRV were verified against the live crate metadata on `2026-09-22`, and the feature set will be pinned when `P3-008` adds it. See `docs/research/integrations/mcp.md`.
| OpenAI Agents SDK | https://github.com/openai/openai-agents-python and https://openai.github.io/openai-agents-python/llms.txt | Runner boundary, tool approvals, guardrails, sessions, tracing, realtime, deterministic testing |
| LangGraph | https://github.com/langchain-ai/langgraph and https://docs.langchain.com/oss/python/langgraph/llms.txt | Checkpoints, interrupts, resume, durability modes, stateful/stateless subgraphs |
| Home Assistant Core | https://github.com/home-assistant/core | Manifest-driven integrations, setup/config flows, diagnostics, quality gates, colocated integration tests |
| ElevenLabs | https://elevenlabs.io/docs/llms.txt | Custom LLM SSE endpoints, MCP consumer mode, voice/telephony, signed post-call webhooks, turn-taking configuration |
| Deepgram | https://developers.deepgram.com/llms.txt | Flux streaming model events, model-integrated end-of-turn thresholds, eager end-of-turn cost |
| LiveKit | https://docs.livekit.io/llms.txt | Agents runtime boundary, telephony, turn-detection and interruption events, simulation-based testing |
| Twilio | https://www.twilio.com/docs | Conversation Relay, Media Streams, speech-model selection that moves turn detection provider-side |
| Temporal | https://docs.temporal.io/llms.txt | Deterministic workflows, activities, retries, signals, timers, worker versioning; deferred adapter |

These are design references, not dependencies selected by this document. The detailed findings and adopt/avoid decisions are in [upstream-patterns.md](docs/research/upstream-patterns.md).

## Dependency Intake

Before adding a dependency:

1. Verify the official package and repository.
2. Record selected version, license, maintenance status, advisories, features, transitive impact, and reason in the relevant research record or ADR.
3. Prefer a narrow feature set and disable unused defaults.
4. Add automated license and vulnerability checks.
5. Define how the dependency is removed or isolated if it becomes unavailable.

The root JARVIS license remains undecided; release packaging is blocked until it is selected.