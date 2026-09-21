# ADR-0002: Use A Daemon With Thin Clients

- Status: Accepted
- Date: 2026-09-20

## Context

JARVIS needs CLI, desktop, web, mobile, voice, telephony, and external-agent surfaces while maintaining one coherent session, memory, approval, event, and workflow state. Putting logic in each interface would duplicate authority and make background work dependent on a window.

## Decision

Run one long-lived `jarvisd` control-plane process per local profile/trust domain. `jarvis`, Tauri, web/mobile, voice, and external clients communicate over authenticated versioned protocols. Clients render, capture user input, and present approvals; they do not own canonical business state.

Local clients prefer Unix sockets or Windows named pipes. Network endpoints bind to loopback by default. Remote mode is explicit.

## Consequences

- Closing a UI does not stop workflows or lose state.
- Every interface observes the same runs, approvals, memory, and audit.
- Client/daemon protocol compatibility and service lifecycle become product requirements.
- Installation must diagnose duplicate daemons, profile mismatch, stale services, and socket/port conflicts.
- A daemon compromise is high impact, so it is the primary hardened trust boundary.

## Alternatives

- One desktop process: simpler prototype, unsuitable for headless/background/multi-client use.
- One process per interface: creates split-brain state and duplicated connectors.
- Cloud-only service: violates local-first and offline/privacy goals.

## Revisit When

Multi-tenant server scale may require multiple daemon replicas, but they must preserve one logical control plane through database coordination and explicit tenancy.