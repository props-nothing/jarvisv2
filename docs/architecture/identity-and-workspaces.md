# Identity And Workspaces

## Trust Domain

One local JARVIS profile is one trust domain and normally one `jarvisd`. Server deployments may host multiple workspaces, but every request and stored row remains explicitly scoped. Profiles never share an implicit global memory or credential pool.

## Identity Types

- **User:** human owner/member.
- **Actor:** authenticated principal responsible for a command or decision.
- **Client:** CLI, desktop, browser, mobile, voice provider, MCP client, runtime worker, or service identity.
- **Device:** enrolled machine/phone associated with one or more clients.
- **Connector account:** verified identity at an external provider.
- **Runtime instance:** supervised execution process/endpoint with no human authority by itself.
- **Guest/caller:** restricted transient identity with unverified or limited evidence.

Do not overload email address, phone number, provider account ID, device name, or session ID as a JARVIS user identity.

## Request Identity

Every application command is evaluated with:

```text
actor
client/device
user when applicable
workspace
authentication method and strength
granted scopes/capabilities
session/channel
correlation/trace
```

The server derives this context from authentication and server-owned mappings. Request payload fields cannot replace it.

## Local Clients

Secure local IPC and user-only permissions establish a strong transport boundary but still use profile/client identity for audit and least privilege. A background process under the same OS account should not automatically gain administrative, secret-export, or destructive authority.

Desktop/CLI bootstrap can use a one-time pairing secret written/read through an OS-protected channel, then rotate to a client credential or key pair. Exact protocol is a Phase 1/remote-auth ADR.

## Remote Clients And Devices

Remote access is disabled by default. Enrollment requires an authenticated owner action and binds a revocable client/device credential. Credentials are scoped, expiring where practical, rotatable, and listed in the UI/CLI with last use.

Pairing proves control of the joining device credential; a QR/setup code is short-lived, single-use, and contains no durable bearer secret in logs/history.

## Workspaces

A workspace scopes:

- members and roles
- memory, conversations, documents, entities, and projects
- connector accounts and secret references
- tool grants and policy
- runtimes/model policy
- events, schedules, workflows, and notifications
- voice sessions/calls
- audit visibility and retention

Personal, company, client, development, and home contexts use separate workspaces when their data or authority should not mix.

## Authorization Model

Use capabilities/scopes with optional roles as bundles. Evaluate:

- actor status and membership
- client/device grant and channel ceiling
- workspace policy
- capability and resource constraint
- connector account ownership/scope
- tool effect/risk and target
- authentication strength/freshness
- approval obligation

Deny overrides allow. A role grants a maximum; a client/channel may have a narrower ceiling. External runtime/MCP/voice identities never inherit the initiating user's entire capability set.

## Session And Delegation

A session binds workspace, user/guest, channel, and client. Cross-channel continuation requires an authenticated explicit binding, not a matching display name.

When an agent delegates to a subagent/runtime, JARVIS mints a narrower execution grant with run, workspace, allowed tools/resources, expiry, and parent causation. Delegation cannot increase authority.

## Connector Identity

After OAuth/account setup, query the provider's authoritative identity endpoint and store its scoped provider ID. Labels entered by a user are presentation only. Reauthorization must prove it is the same intended account or require an explicit replacement/migration.

## Voice Identity

Caller ID, custom SIP headers, ElevenLabs dynamic variables, and a provider `user_id` are evidence from the voice channel. Map only values established during trusted setup. Unknown callers get a guest session and cannot access personal memory or effectful tools without stronger verification.

## Tests

- cross-workspace repository and retrieval tests
- client grant ceilings and deny precedence
- revoked/expired/stale credential tests
- session continuation cannot change workspace silently
- delegation cannot broaden scopes
- OAuth callback cannot bind a different provider account silently
- forged voice metadata remains guest
- approval requires eligible actor and sufficient auth strength
- diagnostics never reveal existence of unauthorized workspace resources