---
integration: web-fetch
status: implemented
last_verified: 2026-10-03
owners: []
selected_spec_version: IANA special-purpose address registries (accessed 2026-10-03)
selected_sdk: reqwest 0.13.5 (already pinned in the workspace; no new HTTP stack)
---

# Web Fetch (`jarvis.web.fetch`)

## Scope

One native tool: an HTTP(S) `GET` of a public URL whose body is returned to the model as **fenced, untrusted text**.

Out of scope: `POST`/non-`GET` methods, request headers or cookies chosen by the model, authentication, a browser or
JavaScript engine, file downloads, search, and any proxy support.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` | not applicable; there is no vendor. IANA and the IETF publish registries and RFCs, not an `llms.txt` | 2026-10-03 | discovery |
| IANA IPv4 Special-Purpose Address Registry | `https://www.iana.org/assignments/iana-ipv4-special-registry/iana-ipv4-special-registry-1.csv` | 2026-10-03 | which IPv4 blocks are not globally reachable |
| IANA IPv6 Special-Purpose Address Registry | `https://www.iana.org/assignments/iana-ipv6-special-registry/iana-ipv6-special-registry-1.csv` | 2026-10-03 | same, for IPv6, including embedded-IPv4 forms |
| `reqwest` 0.13.5 source | `~/.cargo/registry/src/*/reqwest-0.13.5/src/async_impl/client.rs` (the pinned crate, read locally) | 2026-10-03 | `ClientBuilder::{redirect, resolve_to_addrs, no_proxy, timeout, connect_timeout}`; default features |
| `reqwest` 0.13.5 docs | `https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html` | 2026-10-03 | cross-check of the API above |
| Workspace `Cargo.toml` | `reqwest = { version = "=0.13.5", default-features = false, features = ["http2", "json", "rustls", "stream"] }` | 2026-10-03 | which features are on |

## Verified Contract

### Facts about the selected crate

- `ClientBuilder::resolve_to_addrs(domain, &[SocketAddr])` overrides DNS for a name. "Ports in the URL itself will
  always be used instead of the port in the overridden addr." The connection is therefore made to the addresses JARVIS
  validated, not to a second lookup — which is what defeats DNS rebinding between a check and a connect.
- `ClientBuilder::redirect(Policy::none())` disables automatic redirects, so every hop can be re-validated by the caller.
- `ClientBuilder::no_proxy()` disables system proxies. A proxy would resolve names itself and make the address check
  meaningless.
- `gzip`, `brotli`, `zstd` and `deflate` are **not** enabled by the workspace's feature list, so reqwest sends no
  `Accept-Encoding` and does not decompress. There is no decompression-bomb surface to bound.
- `timeout`, `connect_timeout` and `read_timeout` exist.
- `ClientBuilder` DNS overrides are keyed by lower-cased domain, and an IP-literal host never consults DNS at all, so a
  literal-IP URL **must be checked as an address**, not only resolved.

### Facts about addresses (IANA registries)

The IPv4 blocks whose `Globally Reachable` column is `False` (or whose entry is reserved) include `0.0.0.0/8`,
`10.0.0.0/8`, `100.64.0.0/10`, `127.0.0.0/8`, `169.254.0.0/16` (link-local, includes the cloud metadata address),
`172.16.0.0/12`, `192.0.0.0/24`, `192.0.2.0/24`, `192.168.0.0/16`, `198.18.0.0/15`, `198.51.100.0/24`,
`203.0.113.0/24`, `240.0.0.0/4`, and `255.255.255.255/32`.

The IPv6 registry adds `::1/128`, `::/128`, `::ffff:0:0/96` (IPv4-mapped, which **embeds** an IPv4 address),
`64:ff9b::/96` (NAT64, **embeds** one), `64:ff9b:1::/48`, `100::/64`, `2001::/32` (Teredo), `2001:2::/48`, `2001:db8::/32`,
`2002::/16` (6to4, **embeds** one), `3fff::/20`, `fc00::/7` (unique local), and `fe80::/10`. Multicast (`224.0.0.0/4`,
`ff00::/8`) is in a different registry and is not forwardable to a unicast destination.

Some registry blocks are *globally reachable* (AS112, the PCP and TURN anycast addresses). They are public, so the
guard does not refuse them.

### Limits And Failure Semantics

A `GET` has no provider-side idempotency key and does not need one: it is safe and repeatable. A transport failure
before any response is `RefusedBeforeReaching` only when the guard refused it (nothing was sent); once a connection was
attempted the honest classification is a provider-unreachable failure. There is no provider request id.

## JARVIS Mapping

| Concept | Value |
| --- | --- |
| Tool id | `jarvis.web.fetch`, version `1.0.0` |
| Effect | `read_only` |
| Risk | `2` (above the `read_only` floor of 0, deliberately — see Decisions) |
| Scope | `web.fetch` |
| Approval | `policy` — held by the default workspace policy (`approval_threshold = "moderate"`) |
| Sensitivity | `internal` |

## Decisions

1. **The guard runs on addresses, after resolution, and every redirect hop is re-validated.** A name is resolved with
   the system resolver, **every** returned address must be public (one private answer refuses the host, so a mixed
   answer cannot be used to pick), and the client is built per hop with `resolve_to_addrs` pinned to exactly those
   addresses. An IP-literal host is validated directly.
2. **Only `http` and `https`, no userinfo, only ports 80 and 443.** Restricting the port removes the tool's use as a
   port scanner of public hosts. A different port needs an operator decision, not a model argument.
3. **No model-chosen headers, no cookies, no proxy, no decompression.** The request is a bare `GET` plus a fixed
   `User-Agent` and `Accept`.
4. **At most 3 redirects, each validated; a redirect to a refused target ends the call with a refusal.**
5. **The body is read as a bounded stream and the read stops at the cap**, so a hostile server cannot make JARVIS
   allocate its content. Only textual content types are returned (`text/*`, JSON, XML, the `+json`/`+xml` suffixes);
   anything else is reported by type and size, never decoded.
6. **The returned text is untrusted and is fenced** with `jarvis_core::IsolatedText` before it leaves the adapter
   (`ADR-0049`). HTML is reduced to visible text by a small tag stripper that drops `script`, `style` and comments.
7. **Risk 2 with `Policy` approval, so the default workspace holds every fetch for a person.** A fetch is a *read*,
   but the model chooses the URL and a URL carries data to the destination (path, query), so a prompt-injected page
   can ask for an exfiltrating fetch. `WorkspacePolicy` can only **tighten** an approval (`ADR-0122`), so the default
   has to be the safe one. An operator who accepts that trade opts in by raising `policy.approval_threshold` to
   `"high"`; the tool is then autonomous. See the limit below.

## Rejected Alternatives

- **A custom `dns::Resolve` filter on the reqwest client.** It protects redirects automatically but is bypassed by an
  IP-literal host, and the validation then lives in two places. Pinning per hop keeps one rule.
- **Checking the name before the request and then letting reqwest resolve again.** Classic rebinding gap.
- **Following redirects with reqwest's policy.** The policy callback sees a URL, not a resolved address.
- **`ExternalCommunication` as the effect.** That effect forces approval through a workspace flag the operator cannot
  relax, which would make the tool permanently non-autonomous.
- **A `readability`-style HTML library.** A new dependency for a feature a stripper covers; revisit on evidence.

## Verification Plan

- Unit: every blocked and allowed address class above, including `::ffff:` mapping, NAT64, 6to4 and Teredo embedding.
- Unit: URL refusals (scheme, userinfo, port, empty host, over-long).
- **Discriminating test:** a loopback listener that records whether a connection arrived. The default policy must
  refuse `http://127.0.0.1:<port>/` **and** a name resolving to loopback **and** a redirect from an allowed target to
  loopback, and the listener must see **zero** connections. Mutating the redirect re-validation makes it fail.
- Unit: the body cap stops reading (a server that streams forever), a non-text type is reported and not decoded, and
  the fence cannot be closed from inside the page.
- Not verified here: live internet fetches. They need no credential, but a test depending on an external site would be
  flaky and is not part of the gate.

## Unresolved Questions

1. **Exfiltration through the URL is not preventable by the tool.** `ADR-0133` moved the fetch from held-by-default
   to risk 1 (runs by default; a URL is capped at 2,048 characters, which bounds each call); `"jarvis.web.fetch" = "ask"`
   under `[policy.approval]` restores the hold. The residual risk is an operator's informed choice. Blocks: nothing, but it must stay visible in `docs/architecture/security.md`.
2. **`http` is allowed.** A plain-HTTP response can be altered in transit. The result says which scheme was used, and
   the output is untrusted either way. Revisit with an `https_only` setting if operators ask.
3. **The model sees at most 4,000 characters of page text** because the executor truncates every tool result to 12,000
   characters and the fence must survive intact. Pagination (`offset`) is a follow-up.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-10-03 | reqwest 0.13.5 (local source), IANA IPv4/IPv6 special-purpose registries | first record | JARVIS engineering |
