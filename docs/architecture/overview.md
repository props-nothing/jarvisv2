# Architecture Overview

## Decision Summary

JARVIS is a native daemon with multiple clients. Rust owns durable orchestration and authority. External models, agent runtimes, voice systems, tools, and databases connect through explicit ports.

The architecture optimizes for five properties:

1. **User authority:** the model cannot grant itself permission.
2. **Replaceability:** no provider or agent framework becomes the platform.
3. **Durability:** accepted work has an identity and recoverable state.
4. **Local-first installation:** one native install can run with SQLite and no infrastructure fleet.
5. **Inspectable behavior:** users and operators can see sources, decisions, effects, and outcomes.

## Process Topology

```mermaid
flowchart TB
    subgraph Clients
        CLI[jarvis CLI]
        Desktop[Tauri desktop]
        Web[Web/mobile client]
        Voice[Local voice or telephony provider]
        Agent[External MCP/API client]
    end

    subgraph Daemon[jarvisd - trusted control plane]
        Gateway[Auth, sessions, HTTP/WS/SSE/local IPC]
        Application[Application use cases]
        Native[Native agent state machine]
        Context[Context engine]
        Policy[Policy and approvals]
        Tools[Tool gateway and MCP host/server]
        Memory[Memory manager]
        Workflows[Events, scheduler, workflows]
        RuntimeMgr[Runtime supervisor/router]
        Audit[Audit and telemetry]
    end

    subgraph Isolated[Replaceable or isolated adapters]
        Models[Model providers]
        Runtimes[OpenClaw / OpenAI Agents / LangGraph / ACP]
        Connectors[Gmail / Microsoft Graph / GitHub / Home Assistant]
        MCP[MCP servers]
        Sandbox[Sandbox workers]
    end

    subgraph Data
        Local[SQLite + local object files + OS keychain]
        Server[PostgreSQL + pgvector + object storage + secret manager]
    end

    Clients --> Gateway
    Gateway --> Application
    Application --> Native
    Application --> Context
    Application --> Policy
    Application --> Tools
    Application --> Memory
    Application --> Workflows
    Application --> RuntimeMgr
    Application --> Audit
    RuntimeMgr --> Runtimes
    Native --> Models
    Runtimes --> Models
    Tools --> Connectors
    Tools --> MCP
    Tools --> Sandbox
    Memory --> Local
    Workflows --> Local
    Audit --> Local
    Memory --> Server
    Workflows --> Server
    Audit --> Server
```

`jarvisd` is a trust boundary, not just a web server. Every client is authenticated according to its transport. The desktop app is not implicitly trusted merely because it runs on the same machine.

## Component Ownership

| Component | Owns | Must not own |
| --- | --- | --- |
| Gateway | client auth, request validation, protocol adaptation, streams | business policy, provider types |
| Application | use-case sequencing, transactions, state transitions | HTTP details, SQL queries, SDK payloads |
| Domain core | identities, entities, policies, state machines, ports, invariants | Axum, SQLx, Tauri, provider SDKs |
| Native runtime | think/act/observe loop and normalized run events | final authorization, raw credentials |
| Runtime manager | selection, process supervision, runtime protocol, health | canonical memory or permission ownership |
| Model gateway | provider normalization, routing, usage, retry classification | agent framework state, tool authorization |
| Tool gateway | registry, validation, grants, policy, approvals, execution, outcomes | model reasoning strategy |
| Connector manager | account lifecycle, OAuth, sync/webhooks, provider operations | cross-provider policy |
| Memory manager | admission, provenance, retrieval, correction, retention, deletion | provider-owned chat history as truth |
| Workflow engine | durable steps, waits, retries, schedules, event triggers | model calls or APIs inside deterministic transitions |
| Voice gateway | call/session mapping and provider adaptation | canonical identity, memory, or tool authority |
| Storage adapters | migrations, transactions, queries, indexes | domain decisions |

## Interactive Request Flow

```mermaid
sequenceDiagram
    participant U as User/client
    participant G as Gateway
    participant A as Application
    participant C as Context
    participant R as Runtime
    participant P as Policy
    participant T as Tool adapter
    participant D as Data stores

    U->>G: Authenticated request
    G->>A: Normalized command + actor context
    A->>D: Create session/run
    A->>C: Build bounded, sourced context
    C->>D: Retrieve scoped memory/documents
    A->>R: Start runtime request
    R-->>A: Activity/output or tool request
    A->>P: Authorize normalized tool intent
    alt Approval required
        P->>D: Persist approval and pause run
        A-->>U: Approval requested
        U->>G: Authenticated decision
        G->>A: Resume approval
    end
    A->>T: Execute authorized call with idempotency key
    T-->>A: Normalized outcome and evidence
    A->>D: Persist steps, audit, memory candidates
    A-->>U: Stream activity and final response
```

The runtime never receives a credential. It receives tool descriptions and scoped handles. A tool request returns to the application and policy layers before any effect occurs.

## Proactive Event Flow

```mermaid
sequenceDiagram
    participant S as Trusted source
    participant E as Event ingress
    participant D as Durable inbox/outbox
    participant W as Workflow engine
    participant A as Agent runtime
    participant N as Notification/voice

    S->>E: Signed webhook, schedule, or internal event
    E->>D: Verify, normalize, deduplicate, persist
    D->>W: Lease event
    W->>W: Evaluate deterministic trigger and policy
    opt Reasoning required
        W->>A: Start bounded agent step
        A-->>W: Structured result/tool requests
    end
    W->>D: Persist outcome and next state
    W->>N: Notify or request approved call
```

## Deployment Profiles

### Local personal

- one `jarvisd` per profile
- SQLite with write-ahead logging and local object directory
- OS keychain for secret material
- Unix socket or Windows named pipe for local clients
- loopback HTTP only when needed by browser, OAuth, MCP HTTP, or voice callbacks
- per-user service, never machine-wide by default

### Server

- stateless API replicas only after ownership/locking semantics exist
- PostgreSQL plus pgvector as canonical data store
- object storage for artifacts and optional recordings
- external secret manager
- TLS, explicit trusted-proxy configuration, backups, restore drills, and rate limits

### Portable

- foreground daemon and explicit data directory
- no service registration or global writes
- suitable for evaluation and removable media, with a warning when the directory lacks secure permissions

## Failure Model

- Persist intent before effect and outcome after effect.
- Assume network responses can be lost after a provider accepted an action.
- Distinguish `failed` from `unknown`; never retry an unknown non-idempotent effect automatically.
- Use leases and fencing/optimistic versions for workers.
- Make inbound events at-least-once and handlers idempotent.
- Resume only at explicit persisted boundaries.
- A dead optional adapter degrades a capability; it does not corrupt core state or stop unrelated capabilities.

## Evolution Rules

- Add infrastructure after a measured bottleneck, not in anticipation of one.
- Add a crate when it enforces an ownership boundary, not merely to shorten a file.
- Keep public protocols narrower and more stable than internal events.
- Use adapters for provider quirks; do not generalize a one-provider anomaly into every domain type.
- Record material changes as ADRs and retain migration compatibility tests.