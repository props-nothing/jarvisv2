# Product Requirements

## Product Definition

JARVIS is a user-controlled personal AI operating layer. It receives requests and events, builds bounded context, selects a reasoning runtime and model, requests capabilities through policy-controlled tools, persists durable state and memory, and returns results through the user's chosen interface.

JARVIS is not a chatbot wrapper, an unrestricted automation script, or a rebranding of one agent framework.

## Users And Deployment Modes

### Personal local

One user, one machine, SQLite, OS keychain, local-only daemon, optional cloud or local models, and optional desktop/voice clients. This is the default install.

### Personal server

One user with multiple paired devices, PostgreSQL plus pgvector, TLS, remote access, backups, and stronger operational controls.

### Team

Multiple authenticated users and workspaces with roles, connector ownership, audit access, and tenant isolation. This is post-v1 and must not weaken personal privacy defaults.

## Core User Journeys

1. Install JARVIS without a development toolchain, configure one model, and ask a question.
2. Connect Gmail or Outlook with least-privilege OAuth and search mail.
3. Ask JARVIS to draft an action, review it, approve it, and see an audit receipt.
4. Teach JARVIS a preference, restart it, retrieve the preference with provenance, correct it, and delete it.
5. Schedule or trigger work that waits, retries, requests approval, survives restart, and completes exactly once at the effect boundary.
6. Route one task to the native runtime and another to OpenClaw or another external runtime without changing clients.
7. Let an external MCP client discover only explicitly granted JARVIS tools.
8. Talk to JARVIS locally or by phone, and receive a policy-approved outbound call for a meaningful event.
9. Diagnose installation, provider, connector, runtime, database, and network failures with one command.
10. Export and delete user data without depending on an AI model.

## Functional Requirements

### Installation And Lifecycle

- `FR-INSTALL-001`: Ship native binaries for supported Windows, macOS, and Linux architectures.
- `FR-INSTALL-002`: Core install must not require Node.js, Python, Rust, PostgreSQL, Docker, or a cloud account.
- `FR-INSTALL-003`: Support portable foreground mode and optional per-user background service installation.
- `FR-INSTALL-004`: Install, update, rollback, doctor, backup, and uninstall must be idempotent and preserve user data unless deletion is explicit.
- `FR-INSTALL-005`: Bind network services to loopback by default.

### Identity And Workspaces

- `FR-ID-001`: Authenticate every non-local-trusted client and bind it to an actor, device/client, user, and workspace.
- `FR-ID-002`: Scope memories, connectors, files, tools, events, workflows, and runtime grants by workspace.
- `FR-ID-003`: Support credential rotation, revocation, session expiry, and lost-device response.

### Conversations And Runs

- `FR-RUN-001`: Persist sessions, messages, runs, steps, model calls, tool calls, approvals, artifacts, usage, and terminal outcomes.
- `FR-RUN-002`: Stream normalized activity and output events without exposing hidden chain-of-thought.
- `FR-RUN-003`: Support cancellation, timeouts, retries, resumable waits, and crash recovery at documented boundaries.
- `FR-RUN-004`: Preserve a stable run identity across runtime handoff and restart.

### Models And Runtimes

- `FR-MODEL-001`: Separate provider, model, and agent-runtime concepts.
- `FR-MODEL-002`: Route using capability, privacy, availability, latency, cost, context, modality, and user policy.
- `FR-MODEL-003`: Normalize provider errors and usage while retaining redacted provider diagnostics.
- `FR-RUNTIME-001`: Support native and out-of-process runtimes through a versioned protocol.
- `FR-RUNTIME-002`: A runtime crash or protocol violation must not crash `jarvisd` or bypass policy.
- `FR-RUNTIME-003`: JARVIS remains usable when every optional runtime is unavailable.

### Tools And Connectors

- `FR-TOOL-001`: Represent every capability as a typed, versioned tool with input/output schema, effects, risk, scopes, timeout, retry, and source.
- `FR-TOOL-002`: Validate and authorize calls immediately before execution.
- `FR-TOOL-003`: Require durable approval according to deterministic policy; models cannot self-approve.
- `FR-TOOL-004`: Distinguish requested, accepted, submitted, provider-confirmed, failed, and unknown outcomes.
- `FR-TOOL-005`: Support MCP as client/host and scoped server over stdio and Streamable HTTP.
- `FR-CONNECT-001`: Support OAuth refresh, reauth, revocation, pagination, incremental sync, webhooks, quotas, and redacted diagnostics.

### Memory And Context

- `FR-MEM-001`: Support working, conversation, episodic, semantic, preference, relationship, and procedural memory.
- `FR-MEM-002`: Store provenance, confidence, validity, sensitivity, creation/update/access times, and supersession.
- `FR-MEM-003`: Keep uncertain inference distinct from user-confirmed fact.
- `FR-MEM-004`: Combine exact/full-text, semantic, recency, importance, entity, relationship, and task relevance.
- `FR-MEM-005`: Explain memory use and support inspect, correct, forget, export, retention, and deletion.
- `FR-CONTEXT-001`: Allocate context by source-specific budget and record why each item was included.

### Events And Workflows

- `FR-EVENT-001`: Admit authenticated events durably with schema version, source, causation, correlation, sensitivity, and dedupe identity.
- `FR-WORKFLOW-001`: Support sequential/parallel steps, dependencies, conditions, retries, delays, schedules, event waits, approvals, cancellation, and compensation metadata.
- `FR-WORKFLOW-002`: Recover waits and in-progress work after process restart without duplicating external effects.
- `FR-PROACTIVE-001`: Respect quiet hours, frequency budgets, relevance, deduplication, channel preference, and user disable controls.

### Voice And Telephony

- `FR-VOICE-001`: Keep voice providers replaceable behind a provider-neutral contract.
- `FR-VOICE-002`: Support local voice clients, inbound calls, and policy-controlled outbound calls.
- `FR-VOICE-003`: Support ElevenLabs with JARVIS as the brain through current OpenAI-compatible streaming endpoints.
- `FR-VOICE-004`: Optionally expose a least-privilege MCP profile to an ElevenLabs-owned agent.
- `FR-VOICE-005`: Correlate calls, sessions, transcripts, tools, approvals, and webhook events without trusting caller-supplied identity.
- `FR-VOICE-006`: Make recording, transcript, consent/disclosure, retention, and deletion policies explicit.
- `FR-VOICE-007`: Default a realtime session to one authenticated caller and audio only. Additional participants and non-audio media (camera, screen share) are opt-in per session, recorded as explicit session scope, and never inherit a previous session's grants.

### Interfaces And Operations

- `FR-API-001`: Expose versioned HTTP and streaming APIs with generated OpenAPI.
- `FR-API-002`: Support a stable local daemon protocol for CLI and desktop clients.
- `FR-OPS-001`: Provide health, status, doctor, logs, metrics, traces, sanitized diagnostics, backup, and restore.
- `FR-OPS-002`: Carry request, run, workflow, model-call, tool-call, session, call, and trace correlation IDs.

## Security Requirements

- `SR-001`: Deny by default when identity, policy, approval, or scope is ambiguous.
- `SR-002`: Never expose raw secrets to models, prompts, normal logs, or client-visible errors.
- `SR-003`: Treat retrieved content and tool output as data, not authority.
- `SR-004`: Prevent path traversal, symlink escape, SSRF, unsafe redirects, archive bombs, command injection, and unbounded output at adapter boundaries.
- `SR-005`: Sandbox arbitrary code and constrain filesystem, network, process, CPU, memory, and time.
- `SR-006`: Preserve immutable decision/audit receipts and redact sensitive payloads by policy.
- `SR-007`: Verify webhook signatures over raw bodies and reject replay.
- `SR-008`: Encrypt transport and sensitive stored data according to deployment mode.
- `SR-009`: Support complete user-data export and deletion, including derived indexes and provider-side cleanup instructions.

## Non-Functional Requirements

- `NFR-PORT-001`: Native behavior must be tested on Windows, macOS, and Linux rather than inferred from one OS.
- `NFR-REL-001`: Durable state transitions are transactional, replayable, and observable.
- `NFR-PERF-001`: Text streaming should begin as soon as useful output exists; voice paths track time to first audible response and interruption latency.
- `NFR-PERF-002`: Voice turn detection is measured and reported separately from perceived response latency, because the two are additive; any latency metric that begins at the final transcript cannot see the turn-detection cost.
- `NFR-PRIV-001`: Local mode operates without JARVIS-hosted telemetry and documents every configured external data flow.
- `NFR-PRIV-002`: A session records which participants and media types were present, so consent, retention, and deletion decisions are attributable to the session that produced them.
- `NFR-OBS-001`: Logs are structured, bounded, correlated, and secret-redacted.
- `NFR-COMPAT-001`: Public protocols and persisted schemas are versioned with migration and compatibility tests.
- `NFR-TEST-001`: Core orchestration is deterministically testable without network credentials or model calls.
- `NFR-ACCESS-001`: CLI and desktop workflows expose equivalent critical controls, including approvals and data deletion.
- `NFR-I18N-001`: Text is Unicode-safe; language behavior follows current input and user preference rather than stale inferred history.

## V1 Scope

V1 is one local user, one machine, SQLite, one OpenAI-compatible model provider, CLI, daemon, safe tools, MCP client/server, canonical memory, one read-only Google or Microsoft connector, events, durable approvals, and installable native artifacts. Desktop and telephony may ship after the headless contracts are proven.

## Non-Goals For V1

- replacing every external agent framework
- unrestricted autonomous computer control
- enterprise multi-tenancy
- Kubernetes or a distributed event platform
- real-time full-desktop video surveillance
- ambient camera or screen capture without a user-initiated, per-session share
- unauthenticated multi-party voice or video rooms
- medical, emergency, legal, or financial decision authority
- claiming an external action succeeded without provider evidence

## Product Success Criteria

- A non-developer completes first install and first response without reading source code.
- A failed connector, model, runtime, migration, or service start yields a specific `doctor` finding and recovery action.
- Every external effect is attributable to an actor, policy decision, tool version, and outcome.
- Every durable memory is inspectable, sourced, correctable, and deletable.
- The same user workflow works through CLI and at least one graphical or voice interface.
- Killing the daemon during a wait or approval does not lose or duplicate work.

Concrete end-to-end scenarios live in [acceptance-tests.md](../quality/acceptance-tests.md).