# Glossary

Use these terms consistently in code, schema, APIs, docs, and UI.

| Term | Meaning |
| --- | --- |
| Actor | Authenticated principal causing or approving an operation: user, service, or constrained system actor. |
| Agent | A reasoning configuration/persona using a runtime, model policy, tools, and context policy. Not the daemon. |
| Agent run | One durable attempt to fulfill an objective, with state, steps, model/tool calls, events, and outcome. |
| Approval | Authenticated human/system-policy decision bound to one exact current intent; never a model assertion. |
| Artifact | File/media/result produced or consumed by a run, tool, workflow, or call and stored by reference. |
| Backchannel | A short acknowledgement ("uh-huh") that must not be treated as a turn or an interruption. |
| Capability | An operation a component can support; availability does not imply permission. |
| Channel | User-facing route such as CLI, web, Slack, phone, or local voice. |
| Client | Software identity connecting to `jarvisd`, such as CLI, desktop, MCP consumer, or voice service. |
| Connector | Adapter managing an external service account/API, auth lifecycle, webhooks/sync, diagnostics, and tools. |
| Context | Bounded, policy-eligible information assembled for one model/runtime call with provenance and inclusion reason. |
| Control plane | Trusted Rust code that owns policy, durable state, routing, lifecycle, and audit. |
| Eager end-of-turn | A speculative turn boundary that permits starting work whose result is a discardable draft. |
| Effect | Observable change or disclosure caused by a tool, such as write, communication, destruction, execution, financial, or privileged action. |
| End-of-turn confidence | Provider-reported confidence for a turn boundary, retained as evidence and not comparable across providers. |
| Event | Versioned immutable fact admitted into JARVIS with source, time, workspace, correlation, dedupe, and payload. |
| Extension | Separately packaged capability provider, normally connected by MCP, HTTP, stdio, or WASI. |
| False interruption | Speech that stopped agent playback without being a real interruption, for example a cough; playback resumes. |
| Memory | Durable sourced claim about the user/world with type, confidence, validity, sensitivity, and lifecycle. |
| Model | A specific reasoning/generation model with capabilities and routing metadata. |
| Model provider | Service or local endpoint that authenticates/transports model requests. |
| Outcome | Normalized evidence state such as submitted, confirmed, failed, unknown, or cancelled. |
| Policy | Deterministic rules deciding whether an actor/client/workspace may perform an effect and what obligations apply. |
| Profile | One local daemon/config/data/service identity. Multiple profiles on one host are isolated installations. |
| Runtime | Engine controlling an agent execution strategy, such as JARVIS Native, OpenClaw, OpenAI Agents, LangGraph, or ACP. |
| Runtime binding | Opaque link from a JARVIS run/session to runtime-native state; not canonical memory. |
| SecretRef | Identifier for secret material held by a keychain/secret manager; not the secret value. |
| Session | User/channel conversation container that may contain multiple runs. |
| Skill | Versioned reusable procedure composed from tools, policy, context, and workflow steps. |
| Tool | Typed atomic capability requested by a model/runtime and executed only through the JARVIS gateway. |
| Tool intent | Canonical immutable request to invoke a specific tool/version with validated arguments and context. |
| Turn | One unit of caller speech ending at a detected boundary; the boundary is a JARVIS event, not merely the last transcript received. |
| Turn detection | Provider-neutral capability that predicts the end of a turn from the meaning of speech. It is model-based, never a silence window ([ADR-0010](adr/0010-model-based-turn-detection.md)). |
| Voice provider | Replaceable STT/TTS/realtime/telephony adapter. It is not the JARVIS brain. |
| Workflow | Versioned durable orchestration of deterministic state and external activities, triggers, waits, approvals, and retries. |
| Workspace | Primary data/authority partition for memory, connectors, files, tools, runs, and policy. |

## Common Distinctions

- **Model versus runtime:** a model generates; a runtime decides how an agent loop uses models and tools.
- **Tool versus skill:** a tool is atomic; a skill is a reusable procedure.
- **Session versus run:** a session is conversational continuity; a run pursues one objective.
- **Memory versus context:** memory is stored canonical knowledge; context is the selected view supplied to one call.
- **Authentication versus authorization:** authentication establishes identity; authorization evaluates allowed action.
- **Provider accepted versus effect confirmed:** request acceptance is not proof of delivery/completion.
- **Runtime checkpoint versus JARVIS workflow:** a runtime checkpoint resumes its internal agent state; a workflow coordinates durable business steps across systems.
- **Turn detection versus silence timeout:** turn detection predicts the boundary from the meaning of speech and produces the canonical turn event; the silence timeout survives only as a maximum-silence cap that forces a boundary when detection does not.
- **Provider turn advice versus JARVIS turn event:** a provider may report a boundary or a confidence; JARVIS records the canonical turn event and keeps the provider value as evidence.