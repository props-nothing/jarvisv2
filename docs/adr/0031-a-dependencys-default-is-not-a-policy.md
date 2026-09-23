# ADR-0031: A dependency's permissive default is not a policy, and an origin is a tuple

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-009` makes JARVIS an MCP **server** — the first time the daemon is the one being called rather than
the caller. Until now every MCP slice ran as a client, and the client is the easy direction: we choose
the servers, so the trust boundary is one we set.

Researching the server half (live fetch of `2026-07-28/basic/transports/streamable-http.md` and
`basic/authorization.md`, `docs/research/integrations/mcp.md`) established the server's MUSTs, and
reading the SDK's server transport alongside them produced the finding that decides this slice's shape.

**`StreamableHttpServerConfig::default()` is permissive on exactly the obligations a security story
rests on.** From `rmcp-3.4.0`'s `transport/streamable_http_server/tower.rs`:

| Field | Default | The MUST it leaves open |
| --- | --- | --- |
| `allowed_origins` | `vec![]`, `validate_empty_origin_allowlist: false` | `validate_origin_header` returns `Ok(())` **before** reading the header, so `Origin` is never checked. The doc comment states it: "Defaults to an empty list, which disables Origin validation for backward compatibility." |
| `legacy_session_mode` | `true` | Mints an `Mcp-Session-Id` per `initialize`, which `2026-07-28` (SEP-2567) removed. |
| `stateless_protocol_metadata_required` | `false` | The doc comment states it "preserv[es] today's legacy behavior where an absent header is treated as protocol version `2025-03-26`" — the opposite of the reject rule. |

The SDK is **not uniformly permissive**: `allowed_hosts` defaults to loopback only, and the `-32020`
header↔body check runs unconditionally before handler dispatch. That is what makes inheritance
dangerous rather than merely disappointing — a fail-closed field and a fail-open field are
indistinguishable at a call site, so "the SDK handles it" is true for the first and false for the second.

A second finding came from reading the comparison rather than its description. The SDK's doc says an
`Origin` "must match per RFC 6454 `(scheme, host, port)`". Its `origin_is_allowed` is:

```text
a_scheme == o_scheme && a_host == o_host && (a_port.is_none() || a_port == o_port)
```

The `a_port.is_none()` arm makes **a portless allowlist entry a wildcard over every port**. Browsers omit
a default port, so an incoming `Origin: https://x` parses with port `None`. The result is that the SDK
gives an operator no way to express the ordinary intent:

- `https://x` matches the default port **and every other port** — a service on `https://x:8443` is
  admitted by an entry that appears to name one origin;
- `https://x:443` matches only a literal `:443` and **false-rejects the normal browser traffic** above.

A control whose narrowest setting is a wildcard and whose exact setting is wrong is not a control.

## Decision

**1. The exposure policy is a JARVIS value built from an operator's configuration, and the permissive
SDK fields are set explicitly.**

`ServerExposure` and `AllowedOrigin` live in `jarvis-mcp` — pure, offline, no SDK in scope. `P3-009b`
maps it onto the SDK's fields with `allowed_origins` from the policy, `enforce_origin_validation()` for
the empty case, `stateless_protocol_metadata_required(true)`, and `legacy_session_mode(false)`.

The reason is not that the defaults are wrong in the abstract but that **a default is the one value that
changes without a line in this repository changing.** An upgrading dependency could reverse any of them,
and a security reviewer reading the call site would see nothing. Stating the value makes each choice
ours, which is the same reasoning that moved the HTTP client into `jarvis-mcp-transport` (`P3-008g`)
rather than trusting the SDK's `default_http_client`.

**2. JARVIS owns the origin comparison, and a default port and an absent port are one origin.**

`AllowedOrigin::matches` applies the scheme's well-known port to each side before comparing. So
`https://x` and `https://x:443` match a browser's `Origin: https://x`, and `https://x:8443` does not.

`PartialEq`, `Eq`, and `Ord` are **hand-implemented to compare the effective port**, because that is what
this type means by equal. This was not the first version: the derive compared `port` as written, and
`the_same_origin_under_two_spellings_is_refused_as_a_duplicate` failed — the duplicate check accepted
`https://x` and `https://x:443` as two entries even though `matches` correctly called them one. A
**found** defect rather than an anticipated one, and the reason the test exists.

**3. An `Origin` comparison is whole-host, never a prefix or suffix test.**

`is_loopback_host` and the host equality both compare the entire name, so `127.0.0.1.evil.example` and
`jarvis.example.com.evil.test` are ordinary remote names. `McpHttpEndpoint::is_loopback` already applies
this rule on the client side; the host side of the same boundary now has it too.

**4. A present but unparseable `Origin` is refused, not treated as absent.**

The specification's rule is about a present and *invalid* value. Folding a parse failure into "absent"
would **invert the control**, because absence is the admitted case — so a malformed header would become
the shape that gets through. `OriginVerdict` therefore distinguishes `Malformed` and `Opaque` from
`Absent` and `NotAllowed`.

**5. Every refusal answers `403`, and the variants exist for the log rather than the wire.**

Telling a hostile caller *why* their origin was refused would help them enumerate the allowlist.
`OriginVerdict::refusal_status` returns the same `403` for every refusal for that reason, and the
variant is what an audit record can use.

**6. An empty allowlist is *enforced*, and the state is named rather than inferred.**

`ServerExposure::loopback_only()` produces an empty allowlist that refuses every present `Origin`, so the
only admitted callers send no `Origin` at all — which is what a local tool does and is a shape that admits
no web page. This is the **opposite** of the SDK's empty default, which admits everything, so the
accessor is `is_loopback_only()` rather than `allowed_origins().is_empty()`: a reader checking only for
emptiness would reach the wrong conclusion about which of the two they hold.

**7. `Origin` is not an authentication boundary, and the type says so.**

A non-browser client sends any `Origin` it likes, and a client that omits the header is admitted by the
specification's own rule. So the allowlist controls *browsers*, and the boundary is
`ServerExposure::requires_remote_bind()` — a refusal to be reachable off-host at all in this slice. A
remotely reachable JARVIS MCP server needs audience-bound tokens (RFC 9728 Protected Resource Metadata
and RFC 8707 resource indicators, both MUSTs for a server that adopts OAuth), and **a network-reachable
MCP server without them would be an unauthenticated control plane.**

**8. `null` is refused at both ends.**

The opaque origin names no origin, so an allowlist entry for it would admit every sandboxed frame and every
`file://` page at once. It is a configuration error *and* its own verdict.

**9. JARVIS authorization stays in front of the MCP server.**

MCP authorization is transport-level and optional; JARVIS scopes, approvals, audit, and idempotency are
not. A remote MCP client is an **actor** in the existing model, so its calls pass the same pipeline. The
MCP layer never becomes a second authorization mechanism, which is the same rule
`docs/research/integrations/mcp.md` already records for the client direction.

## Consequences

- **The finding is reusable beyond MCP.** "A dependency's `Default` is a claim, not a decision" is the
  third instance of this shape in this project (`default_http_client` and proxy, the SDK's `LATEST`
  version constant, and now the server config) — and the first where the finding is a *diff* rather than
  a missing feature. The pattern to watch for is a dependency whose defaults are strict in one field and
  permissive in another, because the strict fields are what make "it is handled" believable.
- **A real defect was found by a test rather than by inspection:** the derived equality stored one origin
  twice. Both halves of the origin rule are now pinned by falsification — replacing the default-port
  implication with `unwrap_or(0)` fails two tests, and folding `Malformed` into `Absent` fails two more.
- **`Origin` semantics are now written down where an operator can read them**, including the two
  spellings that denote one origin. The old behaviour's failure mode was silent in both directions.
- Still **not built**, and recorded rather than implied: no server transport is wired to this policy yet
  (`P3-009b`); no remote bind, therefore no OAuth resource-server; no client allowlist or rate limiting
  (`P3-009c`); and the policy does not yet model `allowed_hosts`, because the SDK's loopback-only default
  is already fail-closed and `P3-009` refuses a remote bind outright.

## Alternatives rejected

- **Inherit the SDK's server defaults and override nothing.** The `Origin` MUST would be unenforced and
  the session and version behaviours would be the legacy ones, with nothing at the call site saying so.
- **Use the SDK's `allowed_origins` comparison.** Its portless-entry wildcard admits any port, and its
  exact-port form false-rejects normal browser traffic. Neither is a setting an operator can use.
- **Compare origins as strings.** `https://x` and `https://x:443` are one origin and would be two, and a
  `contains` test would admit `https://x.evil.example`.
- **Default a missing scheme to `https`.** Guessing would silently admit a scheme the operator did not
  name — the same reasoning that refuses a typo'd posture class rather than defaulting it (ADR-0029).
- **Skip a malformed allowlist entry rather than refusing the configuration.** Silent narrowing, which
  ADR-0020 refuses for a grant and which is equally wrong here.
- **Treat a malformed `Origin` as absent.** Inverts the control, admitting exactly what the check exists
  to catch.
- **Fold `Opaque` into a generic refusal.** It is a distinct condition — a sandboxed frame or a `file://`
  page — and a log that cannot name it cannot explain a wave of refusals.
- **Publish a different status per refusal reason.** Enumerating the allowlist for a hostile caller.
- **Expose the server remotely with JARVIS authorization in front.** JARVIS authorization would be doing
  the work while the transport claimed to do it, and the transport's own MUSTs (audience binding) would
  remain unmet — so the honest shape is not-reachable until the OAuth half exists.
