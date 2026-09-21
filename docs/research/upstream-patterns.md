# Upstream Architecture Patterns

Reviewed on **2026-09-20** from official repositories and documentation. This is a structural study, not a claim that JARVIS has implemented these projects or copied their code.

Agent-framework and telephony platforms were added on **2026-09-21** in the same spirit: they are studied for their structural choices, and the resulting provider records carry the contract detail.

## Decision Filter

Patterns were accepted only when they reinforce JARVIS requirements:

- native cross-platform installability
- one trusted daemon/control plane
- thin clients and stable protocols
- replaceable runtimes/providers
- repairable state and updates
- explicit extension metadata
- deterministic policy and testability

Popularity alone is not an architectural reason.

## OpenClaw

Official sources: https://github.com/openclaw/openclaw and https://docs.openclaw.ai/llms.txt

Observed ownership:

- `src/gateway`: authenticated gateway/API and client flows
- `src/cli`, `src/commands`: command registration and operational flows
- `src/daemon`: platform service planning/install/reconciliation
- `src/plugins`, `src/plugin-sdk`, `extensions`: plugin contracts and separately owned capabilities
- `apps/*`: native/desktop clients
- `docs/gateway`, `docs/cli/doctor`, `docs/install`: operational contracts

Adopt:

- a gateway/daemon as the shared stateful service
- service installation as a planned/reconcilable operation, not a blind file write
- `doctor` checks that separate detection, evidence, repair authority, and post-repair verification
- immutable installed package tree with mutable state/extensions in owned data roots
- versioned plugin/runtime contracts, scoped tools, secret references, and protocol/schema generation
- explicit "installed service versus active binary/config" drift diagnosis

Avoid:

- inheriting OpenClaw's language/runtime requirements into the JARVIS core
- making its sessions/database canonical JARVIS memory
- reproducing its very broad feature surface before core invariants are proven

## Goose

Official source: https://github.com/aaif-goose/goose (the prior `block/goose` URL redirects here).

Observed root layout includes `crates`, `ui`, `bin`, `documentation`, `examples`, installers/download scripts, Rust toolchain/config, release docs, and security/governance files. The project presents desktop, CLI, and API surfaces over a Rust-heavy core and supports MCP extensions and ACP.

Adopt:

- Rust core shared by CLI/API/desktop surfaces
- native release artifacts and shell/PowerShell bootstrap installers
- MCP as an extension boundary and ACP as agent interoperability
- dedicated build/release/security documentation alongside source
- custom distributions as configuration/packaging, not forks of core behavior

Avoid:

- matching its crate count or directory names without a JARVIS ownership need
- assuming a coding-agent loop is sufficient for personal memory, events, telephony, and durable automation

## Tailscale

Official source: https://github.com/tailscale/tailscale

Representative ownership:

- `cmd/tailscaled`: long-lived daemon and platform service entry points
- `cmd/tailscale/cli`: thin command surface
- `client/local`: local daemon client with documented API maturity
- `paths`: platform-specific state/socket paths and restrictive permissions
- `clientupdate`: verified, platform-specific update machinery
- `tstest/integration`: real daemon/service lifecycle tests, including Windows services

Adopt:

- distinct daemon and CLI binaries with a local API
- API maturity/compatibility documented per surface
- settings in mutable profile/config APIs rather than proliferating daemon flags
- platform-specific implementation behind common lifecycle/path contracts
- disposable native integration tests that refuse to damage a pre-existing installation
- start/stop readiness checks and cleanup ordering

Avoid:

- importing its networking complexity or privileged system-daemon posture into local JARVIS v1

## Tauri

Official sources: https://github.com/tauri-apps/tauri and https://v2.tauri.app/llms.txt

Representative ownership:

- `crates/tauri`: application runtime and scoped APIs
- `crates/tauri-cli`: build/development interface
- `crates/tauri-bundler`: OS package formats and updater artifacts
- platform modules guarded by target configuration
- webview/frontend communicates through an explicit IPC/permission surface

Adopt:

- native Windows/macOS/Linux desktop packaging and signed updates
- OS application-data/cache/log path APIs
- a minimal webview permission/command allowlist
- native CI for platform bundles

Avoid:

- placing daemon business logic in Tauri commands
- requiring the desktop app for headless/server use

## Model Context Protocol And Rust SDK

Official sources: https://modelcontextprotocol.io/llms.txt and https://github.com/modelcontextprotocol/rust-sdk

Representative ownership:

- `crates/rmcp/src/model`: protocol data
- `handler/client` and `handler/server`: role behavior
- `transport`: stdio, child process, Streamable HTTP and reusable transport traits
- `examples/clients`, `examples/servers`, `tests/test_with_js`: examples and cross-SDK checks
- `conformance`: evidence about implemented feature coverage

Adopt:

- feature-gated client/server/transports
- protocol negotiation rather than latest-only assumptions
- independent cross-SDK and Inspector tests
- auth and transport concerns at the MCP adapter edge

Avoid:

- MCP types as internal tool/domain types
- assuming every current spec extension is implemented by the selected SDK release

## OpenAI Agents SDK

Official sources: https://github.com/openai/openai-agents-python and https://openai.github.io/openai-agents-python/llms.txt

Representative ownership:

- `src/agents/run.py`: public runner/orchestration entry
- `src/agents/run_internal`: lifecycle, turn resolution, tool execution, resume state
- `src/agents/models`: model/provider interfaces
- `src/agents/mcp`: MCP lifecycle and approvals
- `src/agents/realtime`, `src/agents/voice`: modality-specific sessions
- `src/agents/sandbox`: capability and isolation mechanisms
- `tests` plus provider-neutral scripted models: deterministic orchestration tests

Adopt:

- keep public runners thin and move mechanics behind focused internal modules
- align streaming and non-streaming behavior
- test run, approval, guardrail, resume, realtime, and sandbox boundaries deterministically
- preserve trace sensitivity controls

Use it as an out-of-process runtime. Its sessions and approvals do not replace JARVIS canonical state/policy.

## LangGraph

Official sources: https://github.com/langchain-ai/langgraph and https://docs.langchain.com/oss/python/langgraph/llms.txt

Representative ownership:

- `libs/langgraph/langgraph/graph`: state graph construction
- `libs/langgraph/langgraph/pregel`: execution/runtime
- `libs/checkpoint`: checkpoint interface
- `libs/sdk-py`: remote server SDK and protocol
- tests for interrupts, resume, pending writes, durability modes, subgraphs, replay/fork, and shutdown drain

Adopt as reference:

- explicit state snapshots and resumable interrupts
- separate checkpoint/store abstractions
- tests for duplicate work and pending writes during resume
- explicit durability modes and thread/run identity

Use only as an optional worker for a named graph use case. JARVIS workflows, memory, approvals, and audit remain canonical.

## Voice And Telephony Platforms

Reviewed on **2026-09-21**. These are studied as replaceable adapters and optional runtimes, never as platforms. Contract detail lives in the individual records; this section is the structural read.

Official sources: https://docs.livekit.io/llms.txt, https://elevenlabs.io/docs/llms.txt, https://developers.deepgram.com/llms.txt, and the Twilio TwiML reference.

Observed structural choices worth noting:

- **Every serious vendor replaced the silence window with a turn-detection model.** Twilio routes to Deepgram Flux server-side, Flux exposes its own confidence thresholds, ElevenLabs ships configurable turn settings over a proprietary model, and agent frameworks run a detector by default. Independent convergence is the evidence, not any single vendor's claim.
- **Model-integrated turn detection** is shipped as a distinct model endpoint and event vocabulary (`EndOfTurn`, `EagerEndOfTurn`, `TurnResumed`) rather than a parameter on a transcription model.
- **Interruption is modelled as several events**, including a false interruption that resumes playback — a distinction absent from a silence-window design.
- **Agent frameworks unify the transport**: browser, agent, and phone call all join one room, so agent code is transport-independent. That is a genuinely better shape than per-channel code, and it is why the framework appears in the backlog as a candidate — with its boundary question open.
- **Telephony is decomposed into carrier versus agent runtime.** A SIP carrier is interchangeable across several providers; swapping one is configuration, not code.
- **Testing without a phone call** is a first-class documented capability (text-mode sessions, simulated conversations, evals). This matches `NFR-TEST-001` and is the strongest structural argument for a framework.
- **Bundled model access is not durable.** A model catalogue can silently drop a vendor, which is the argument for own provider accounts over platform bundles.

Adopt:

- the provider-neutral turn/interruption event vocabulary, regardless of provider or framework
- turn-detection latency measured separately from perceived response latency
- explicit turn and interruption policy in the session, with the provider's effective values persisted as evidence
- carrier versus agent-runtime separation, with engine selection made in one place and recorded
- offline, credential-free assertions for agent behaviour, and codec self-tests before any paid call

Avoid:

- inheriting a provider's turn defaults, which are documented to change without notice
- treating a provider buffer-clear or interruption frame as caller intent
- letting a framework own identity, memory, policy, or the canonical transcript
- adopting speech-to-speech as a default without a measured requirement it satisfies
- citing prototype measurement as authority once the prototype is gone

## Home Assistant Core

Official source: https://github.com/home-assistant/core

Representative ownership:

- `homeassistant/components/<domain>/manifest.json`: discoverable static integration metadata
- `config_flow.py`: tested setup/reauth/reconfigure flow
- `coordinator.py` and provider client modules: runtime update ownership
- `diagnostics.py`: intentionally redacted support evidence
- `tests/components/<domain>`: integration-specific configuration, diagnostics, error, and behavior tests
- `script/hassfest`: automated manifest and quality validation

Adopt:

- one manifest-driven connector directory with matching test directory
- setup that tests connectivity and duplicate identity before saving
- reauth/reconfigure as first-class flows
- redacted diagnostics and an automated connector quality checklist
- keep provider protocol/domain logic in a focused client/library rather than UI/setup code

Avoid:

- copying its entity model where JARVIS needs operation-oriented tools

## ElevenLabs

Official source: https://elevenlabs.io/docs/llms.txt

Adopt:

- JARVIS as a Custom LLM brain through OpenAI-compatible SSE
- optional scoped MCP access for an ElevenLabs-owned specialist
- provider-neutral call records and signed webhook reconciliation
- explicit latency, privacy, retention, data-residency, and telephony constraints

Avoid making ElevenLabs memory, identity, approvals, or agent configuration canonical. Details are in [elevenlabs.md](integrations/elevenlabs.md).

## Temporal

Official source: https://docs.temporal.io/llms.txt

Adopt as a future adapter/reference:

- deterministic workflow code and non-deterministic Activities
- durable timers, signals/updates, retries, event history, worker versioning, and graceful worker shutdown
- agent-specific official skills/docs when evaluating implementation

Do not deploy Temporal for local v1. First prove the native database-backed workflow port and adopt Temporal only with measured operational need.

## Resulting JARVIS Layout

The combined lesson is not "copy the largest repository." It is:

- core domain and use cases in a small Rust workspace
- one daemon composition root
- thin CLI/desktop/web/voice clients
- adapter crates for storage, models, tools, connectors, runtimes, and voice
- out-of-process workers for foreign agent frameworks
- manifest-driven extensions/connectors
- migrations, installers, docs, fixtures, and platform tests as first-class product code
- a `doctor` command and acceptance suites that prove the installed system, not just libraries

That ownership is codified in [repository-layout.md](../architecture/repository-layout.md).