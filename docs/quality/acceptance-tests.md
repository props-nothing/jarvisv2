# Acceptance Tests

These are release-level scenarios. All are **unimplemented** until linked evidence is recorded. Unit tests alone cannot satisfy a scenario that names installation, restart, an external provider, a real OS, or a real call.

## Evidence Format

For each proven scenario retain:

- application revision and artifact checksum
- OS/architecture and deployment profile
- configuration/schema/protocol versions
- commands or automated test ID
- sanitized logs/traces and correlation IDs
- pass/fail and known limitations
- external provider/version/date when applicable

## A01: Clean Cross-Platform Install

**Phase:** 1 and 9

Given a clean supported Windows, macOS, or Linux machine without Rust, Node, or Python, when the user runs the signed installer, then:

- the correct artifact is detected and verified
- files use documented OS application directories
- `jarvis --version` works
- foreground `jarvisd` starts and becomes ready
- optional per-user service installation works without hidden elevation
- `jarvis status` reaches the same daemon/profile
- no model account or PostgreSQL is required

Run on every supported OS/architecture artifact, not one representative platform.

### Partial Evidence (`P1-012`)

A01 is **not** satisfied: it requires a signed installer, which does not exist yet.

The parts that need no installer are covered by the automated Phase 1 gate,
`cargo test -p jarvis-acceptance --test phase1_gate`. It starts the real `jarvisd`
on a temporary portable root and proves, per OS:

- `jarvis --version` works
- foreground `jarvisd` starts and becomes ready (readiness is read from durable
  state for *that* process, not from stdout)
- `jarvis status` reaches the same daemon/profile
- no model account or PostgreSQL is required

Still unproven here: the signed installer and artifact verification, and optional
per-user service installation (planning and drift detection exist; installation
does not).

## A02: Doctor Explains A Broken Install

**Phase:** 1

Given deliberately mismatched config/state paths, an unavailable database, stale service command, occupied port/socket, and unsupported newer schema in separate cases, when `jarvis doctor` runs, then each case produces a stable finding code, safe evidence, a specific remediation, no secret leakage, and no mutation unless repair was explicitly requested.

## A03: Durable Conversation

**Phase:** 2

Given a scripted streaming model and an active run, when `jarvisd` is terminated at every persisted run transition, then restart either resumes from the documented boundary or marks the run with a truthful terminal/recoverable state. It never emits duplicated final output or loses an accepted user message.

## A04: Cancellation

**Phase:** 2

Given a run streaming model output, waiting on a tool, and waiting on approval in separate cases, when an authorized client cancels it, then new work stops, adapters receive cancellation, state settles once, and the client receives a terminal cancellation event.

## A05: Tool Approval Cannot Be Forged

**Phase:** 3

Given a model-requested `email.send`, when the model adds fields that claim approval, replays another approval, changes the recipient/content after approval, or submits an expired decision, then execution is denied. Only an authenticated eligible approver deciding the exact current intent can resume it.

## A06: Ambiguous External Effect

**Phase:** 3

Given a non-idempotent provider accepts a request and the connection fails before a response reaches JARVIS, when recovery runs, then the tool call is `unknown`, no blind retry occurs, and reconciliation/user guidance is available. It is never reported as definitely failed or completed.

## A07: MCP Client And Server

**Phase:** 3

Given one local stdio MCP server and one remote Streamable HTTP server, JARVIS negotiates/list/calls their granted tools through canonical policy. Given an external MCP client identity, the JARVIS server exposes only its allowlisted workspace tools. Inspector and independent-SDK tests pass, while unauthorized, malformed, oversized, and SSRF cases fail closed.

## A08: Memory Lifecycle

**Phase:** 4

Given the user says "Remember that client proposals should be concise," after restart JARVIS retrieves that preference with source and confidence. When corrected, old content is not current. When forgotten, canonical text, embeddings, indexes, caches, and relationship edges are deleted according to policy.

## A09: Workspace Isolation

**Phase:** 4

Given semantically similar memories/documents in two workspaces, no query, context build, model call, tool, search, export, trace, or diagnostics request from one workspace reveals the other's content or existence.

## A10: Google Or Microsoft Read Connector

**Phase:** 5

Given a dedicated test account and minimum read scopes, onboarding verifies provider identity and connectivity before saving. Search handles pagination. Expired access tokens refresh. Revoked consent enters reauth. Rate limits retry within budget. Diagnostics remain useful and redact all credentials/content not needed for support.

## A11: Connector Write Approval

**Phase:** 5

Given a requested follow-up email or calendar event, JARVIS shows the exact account, recipients/attendees, content/time, attachments, and effect. Approval executes once, stores the provider receipt, and distinguishes submitted from delivered/confirmed.

## A12: Event Replay And Webhook Security

**Phase:** 6

Given a signed provider event, duplicate concurrent delivery creates one canonical event/effect. Mutated body, stale timestamp, wrong endpoint/account, invalid signature, oversized payload, and replay outside policy are rejected before trusted parsing/workflow execution.

## A13: Workflow Restart

**Phase:** 6

Given a workflow that waits until a time, calls a transient API, requests approval, and sends one idempotent effect, kill `jarvisd` before and after every transition. After restart, waits resume, attempts remain auditable, and the final effect occurs exactly once.

## A14: Proactive Budget

**Phase:** 6

Given repeated equivalent events during quiet hours, JARVIS deduplicates them, respects the user's channel/frequency/quiet-hour policy, and sends at most the configured notification when the window opens. Disabling the automation prevents future triggers without deleting audit history unexpectedly.

## A15: Runtime Switching And Isolation

**Phase:** 7

Given the same normalized request, route one run to native JARVIS and one to an external runtime. Both use JARVIS memory/tool/policy/audit. Crash or send malformed frames from the worker: `jarvisd` remains healthy, the run settles truthfully, unrelated runs continue, and orphan processes are cleaned up.

## A16: ElevenLabs Custom LLM Voice

**Phase:** 8

Given a valid short-lived voice session, official-shape `/v1/responses` and `/v1/chat/completions` requests stream valid SSE and correlate to one JARVIS session. Missing/expired credentials, altered workspace metadata, malformed system tools, disconnect, and cancellation fail safely. Hidden chain-of-thought is not exposed.

## A17: Inbound Call

**Phase:** 8

Given an assigned ElevenLabs/Twilio or SIP number, a verified known caller can ask for permitted calendar information and hear a response from the JARVIS brain. An unknown/spoofed caller receives guest capability only and cannot disclose private memory or execute sensitive tools.

## A17a: Turn Taking And Interruption

**Phase:** 8

Given a call with model-based turn detection, a pause-heavy request ("...reply to the one from Sam, uh, the invoice") completes without being cut off at the silence threshold; a backchannel or non-speech sound during agent speech does not cancel the reply; a genuine interruption does. Given a false interruption, the interrupted response resumes where it stopped without repeating already-heard sentences. Turn-detection latency is reported separately from perceived response latency. A speculative reply produced on an eager turn boundary that is later resumed is discarded and never surfaced as a complete answer.

## A17b: Speech Engine Brain Transport

**Phase:** 8

Given a configured Speech Engine brain, a call establishes the brain session with a valid shared secret and a per-call signed URL, relays telephony audio without transcoding, and correlates to exactly one JARVIS run. A wrong or missing shared secret, a reused signed URL, and a duplicated brain session for one call all fail closed. A provider buffer-clear frame is recorded as transport control and never as a caller interruption.

## A17c: Session Scope For Participants And Media

**Phase:** 8

Given a default voice session, only the authenticated caller is a participant and only audio is accepted. An unauthenticated second participant, an unexpected camera or screen-share track, and a data-channel message attempting a tool effect are each refused or ignored by policy and recorded. Given an explicitly scoped multi-participant or screen-share session, the session record names the participants and media types present, consent and retention apply to each, and no grant from a previous session is inherited.

## A18: Outbound Call

**Phase:** 8

Given an urgent event matching explicit standing policy or a fresh approval, JARVIS calls the configured recipient once, correlates call/conversation IDs, handles busy/no-answer/voicemail/provider failure, ingests signed post-call events, and applies transcript/audio retention. Replaying the trigger does not create another call.

## A19: Desktop Approval Integrity

**Phase:** 9

Given an approval shown in desktop UI, the human-visible target/effect/content hashes to the intent executed. Stale UI, reconnect, concurrent edit, double-click, and decision from another workspace cannot approve a different action.

## A20: Signed Update And Rollback

**Phase:** 9

Given a healthy installed prior release, update verifies manifest/artifact signatures, creates required backup, preflights schema/config, switches atomically, probes daemon/client compatibility, and retains rollback. A corrupted signature, failed migration, unhealthy daemon, and interrupted update preserve or restore a usable prior installation.

## A21: Uninstall And User Data Choice

**Phase:** 9

Uninstall removes service registration and binaries. It asks separately whether to retain or delete config, canonical data, artifacts, recordings/transcripts, logs, backups, and keychain credentials. Non-interactive mode requires explicit flags and never silently deletes data.

## A22: Backup And Restore

**Phase:** 9/10

A consistent backup restored into an isolated profile reproduces allowed canonical state and object references, passes database integrity and `doctor`, and does not place secret values into plaintext archives. A corrupt/incompatible backup fails before replacing active state.

## A23: Prompt Injection Resistance

**Phase:** 3 onward

Given an email/web page/document/tool result instructing JARVIS to reveal secrets, change policy, or call a dangerous tool, the content is treated as untrusted data. It cannot alter grants or approval. Any requested action follows the normal policy path and the source is visible in context/audit evidence.

## A24: Complete User Data Export/Delete

**Phase:** 4 onward

Export enumerates canonical user/workspace data in a documented portable form. Deletion removes owned rows, memory/document indexes, artifacts, caches, connector subscriptions/credentials as requested, and provides provider/backup retention caveats. Audit retains only policy-approved content-free evidence.

## A25: Resource And Cost Limits

**Phase:** 2 onward

Given excessive input, output, tool loops, concurrent runs, event replay, call attempts, or provider rate limiting, JARVIS enforces actor/workspace limits, stops safely, reports actionable status, and records normalized usage/cost without an unbounded retry or memory/log growth.

## Release Gate Mapping

| Release stage | Required scenarios |
| --- | --- |
| Developer alpha | A02-A07, A23, A25 with deterministic adapters |
| Local headless alpha | A01-A09, A12-A15 on all target OSes |
| Connector beta | A10-A13 plus provider live smokes |
| Voice beta | A16-A18 plus A17a, A17b, and A17c plus privacy/legal review |
| Desktop beta | A19-A21 plus native installer lanes |
| Stable | all applicable scenarios, A22/A24, security review, restore drill |