# JARVIS Roadmap

This roadmap is ordered by risk. Each phase must leave a runnable vertical slice. A later phase may start only when the previous phase's exit gate passes or an ADR explains the exception.

## Current Direction (reviewed 2026-10-06)

The phases below remain the dependency order of the *platform*; the order of *product slices* was changed on purpose,
because a platform that cannot yet be talked to, watched or trusted with a task is not the product. Delivered ahead of
their phases, each with an ADR and a live run against a real model: web fetch (`ADR-0129`), approval from the CLI that
survives a restart (`ADR-0130`), code in a disposable container (`ADR-0131`), scheduled tasks (`ADR-0132`), first-run
`init`/`start`, a kill switch, standing trust for low-risk tools (`ADR-0133`), sub-agents (`ADR-0134`), and the browser
console with its holographic head and browser-native voice, served as the daemon's main page (`ADR-0135`). `example/` is
still not retired (`P9-009`).

Measured against the four hallmarks of a real J.A.R.V.I.S.:

| Hallmark | State | Next slice |
| --- | --- | --- |
| Oversight | Strongest. Approvals, `jarvis cancel`, one-level sub-agents, `jarvis watch`, the console. | Coding agents as supervised runtimes (`P7`, via ACP); a push notification when something needs you (`P6-007`). |
| Visual presence | The console at `/` (`jarvis hud`): a holographic human head (real face mesh) that speaks, listens and reacts, streaming conversation, waiting/working/scheduled/recent and "can do" panels with Stop, Approve and Deny; `jarvis start` opens it. | A push stream instead of polling; the Tauri desktop client (`P9-001`). |
| Voice | Browser-native push-to-talk, wake word, spoken answers, interruption (the microphone path is untested here; the browser's recognizer may use a vendor cloud). | Local (non-cloud) recognition and synthesis, then telephony and ElevenLabs (`P8`). |
| Hands-free | Spoken "Jarvis, stop" cancels everything, and a spoken "yes"/"no" answers a waiting approval (`ADR-0136`). | Always-on local wake word and recognition so it works without the browser tab in front. |

Order of work from here: (1) one real connector callable by the model (Gmail/Calendar read), because `jarvis-connectors`
is 38k lines and nothing in `jarvisd` calls it, so `P5` is time-boxed to that single slice; (2) a file write/edit tool for
granted folders; (3) local voice; (4) coding-agent runtimes; (5) model routing so sub-agents and background work can use a
cheaper model; (6) the answer-quality harness (`P9-011`) moved forward, because every change needs a measure of whether
the assistant got better; (7) archive the finished parts of `TODO.md`, which has outgrown being readable as a ledger.
## Phase 0: Architecture Baseline

**Outcome:** future contributors share one product definition, trust model, repository map, research method, and testable backlog.

**Exit gate:** all canonical documents linked from `ARCHITECTURE.md` exist, agree on ownership, and identify the first implementation slice.

## Phase 1: Installable Local Foundation

**Outcome:** a native `jarvisd` and `jarvis` can be built and run with no cloud account.

Deliver:

- Rust workspace and pinned toolchain
- daemon lifecycle, local IPC, health, status, and graceful shutdown
- platform-aware config/data/cache/log paths with restrictive permissions
- versioned configuration and SQLite migrations
- structured logging, correlation IDs, and secret redaction
- `jarvis status`, `jarvis doctor`, and portable foreground mode
- CI on Windows, macOS, and Linux

**Exit gate:** on all three operating systems, a clean build starts the daemon, the CLI reaches health over local transport, state survives restart, and `doctor` diagnoses a deliberately broken configuration.

## Phase 2: First Safe Conversation

**Outcome:** text input produces a streamed response through a provider-neutral model gateway, with durable sessions and runs.

Deliver:

- model/provider contracts and one OpenAI-compatible adapter
- explicit native agent state machine
- session, message, run, step, and model-call persistence
- context budgets and activity events
- cancellation, timeout, retry classification, and usage accounting
- deterministic scripted-model tests

**Exit gate:** a run streams to CLI, survives daemon restart at a persisted boundary, can be cancelled, and is fully testable without network credentials.

## Phase 3: Safe Capabilities And MCP

**Outcome:** JARVIS can use local and remote capabilities without granting the model authority.

Deliver:

- canonical tool schemas, effect classes, risk levels, and grants
- schema validation, policy evaluation, approvals, idempotency, and audit
- MCP client/host for stdio and Streamable HTTP
- scoped MCP server for external clients
- first safe filesystem read tool and sandboxed execution boundary

**Exit gate:** an external MCP client can call only its granted read tool; a write tool pauses for a human decision; restart and duplicate delivery do not execute the effect twice.

## Phase 4: Canonical Memory And Context

**Outcome:** JARVIS remembers deliberately, transparently, and within workspace boundaries.

Deliver:

- typed memory lifecycle with provenance, confidence, correction, expiry, and deletion
- entities and cautious identity resolution
- hybrid full-text, semantic, recency, importance, and relationship retrieval
- embedding provider abstraction and pgvector implementation for server mode
- context selection explanations and token budgets
- memory search, inspect, correct, forget, export, and retention APIs
- procedural memory: a skill format in which a procedure names already-granted tools and never grants
  authority ([ADR-0117](docs/adr/0117-a-skill-is-a-procedure-not-a-permission.md)), its promotion approval,
  and agent-proposed memory admission

**Exit gate:** a remembered preference survives restart, is retrieved with provenance, can be corrected and forgotten, and never crosses workspace boundaries.

## Phase 5: First-Party Connectors

**Outcome:** users can connect real services through auditable, least-privilege accounts.

Implement in this order:

1. Google OAuth, Gmail read, Google Calendar read
2. Microsoft identity, Outlook read, Microsoft Calendar read
3. GitHub read operations
4. Draft and write operations behind approval
5. Generic webhook ingress

**Exit gate:** setup tests connectivity before saving, refresh is resilient, scopes are visible, revocation works, diagnostics are redacted, and contract/live tests prove the supported operations.

## Phase 6: Events And Durable Automation

**Outcome:** JARVIS reacts proactively and resumes long-running work after failure.

Deliver:

- durable event inbox/outbox and deduplication
- scheduler with timezone and daylight-saving behavior
- native workflow state machine with leases, retries, waits, parallel steps, cancellation, and compensation metadata
- skill-candidate and memory-nudge events, and delegation as a step whose effect is starting a child run
- event-triggered skills and notification routing
- approval and timer restart recovery

**Exit gate:** a scheduled workflow pauses for approval, the daemon is killed, and the workflow resumes exactly once after restart.

## Phase 7: Runtime Ecosystem

**Outcome:** external agent engines are replaceable execution strategies.

Deliver:

- versioned JARVIS runtime protocol and supervisor
- capability negotiation, event normalization, cancellation, health, and crash isolation
- delegate/subagent semantics in the runtime protocol, with a parent run recorded and a delegation tool whose effect is starting a run
- OpenClaw adapter
- OpenAI Agents adapter
- ACP adapter
- LangGraph adapter only where a concrete workflow needs it

**Exit gate:** the same client request can be routed to native and external runtimes while JARVIS retains policy, tool, memory, and audit ownership.

## Phase 8: Voice And Telephony

**Outcome:** users can speak to JARVIS, call it, and receive approved calls from it.

Deliver:

- provider-neutral voice session and call contracts
- model-based turn detection and interruption, with turn-detection latency measured separately ([ADR-0010](docs/adr/0010-model-based-turn-detection.md))
- recorded realtime session scope (participants, non-audio media, data channels, E2EE policy) before any realtime transport implementation
- local push-to-talk and wake-word client path
- ElevenLabs Custom LLM support through `/v1/responses` and `/v1/chat/completions`
- scoped JARVIS MCP endpoint for ElevenLabs-owned agents
- inbound identity mapping, Twilio/SIP outbound calls, signed webhooks, transcripts, and retention controls
- interruption, latency, voicemail, and failure-state tests

**Exit gate:** an authenticated inbound call can ask for calendar data through JARVIS, and a policy-approved workflow can place an outbound call with correlated audit and call records. A pause-heavy request completes without being cut off at a silence threshold, and perceived latency is reported separately from turn-detection latency.

## Phase 9: Desktop And Product Installation

**Outcome:** JARVIS feels like an application, not a source checkout.

Deliver:

- Tauri desktop client with chat, activity, approvals, connections, memory, tasks, models, runtimes, voice, and settings
- messaging surfaces as authenticated gateway clients, one per platform
- onboarding and recovery UX
- signed installers, checksums, update channels, rollback, and uninstall
- Linux systemd user unit, macOS LaunchAgent, Windows per-user scheduled task; optional elevated system service modes
- clean-machine installer tests on native CI runners

**Exit gate:** a non-developer can install, configure, use, update, diagnose, and uninstall JARVIS on every supported OS without Node, Python, or Rust.

## Phase 10: Server, Multi-Device, And Scale

**Outcome:** the same core supports an always-on server and multiple trusted devices.

Deliver only when demanded by evidence:

- PostgreSQL plus pgvector production profile
- object storage, backups, restore drills, and disaster recovery
- device pairing, token rotation, revocation, and remote access hardening
- team roles and stronger tenant isolation
- optional Redis, distributed bus, Temporal, or Kubernetes behind existing ports

**Exit gate:** recovery objectives, load targets, isolation tests, and operational ownership are documented and proven.

## Explicitly Deferred

- Kafka, NATS, Redis, Qdrant, Elasticsearch, Kubernetes, and Temporal in the local v1
- unreviewed marketplace plugins
- unrestricted host shell execution
- silent autonomous financial, destructive, or mass-communication actions
- canonical memory owned by an external runtime or provider