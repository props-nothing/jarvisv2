# ADR-0027: A remote MCP endpoint is a validated value, and its HTTP client is built here

**Status:** Accepted

**Date:** 2026-09-23

## Context

`P3-008f` closed the recorded limit that `connect_stdio` was unexercised, and in doing so recorded the
matching one honestly: "**`connect_http` remains entirely unexercised**". It remained unexercised for a
reason that is itself the finding — `connect_http` took a bare `&str`:

```rust
pub async fn connect_http(server: &ServerName, endpoint: &str) -> Result<McpConnection, ConnectError>
```

Every rule about what a remote endpoint may be was therefore **implicit and unenforced**.
`docs/architecture/security.md` lists "SSRF and unsafe redirects" as a threat with the mitigation "URL
parser; scheme/host/IP policy; DNS rebinding defense; redirect revalidation; block metadata/private
ranges by default", and a `&str` satisfies none of them.

Two facts made the gap concrete rather than theoretical, both read from the pinned SDK rather than
assumed:

1. **The SDK's `default_http_client` disables redirects but does not call `no_proxy`.** Its builder is
   `reqwest::Client::builder().pool_max_idle_per_host(0).redirect(Policy::none()).build()`. So proxy
   support is off **only because the SDK's manifest pins `reqwest` with `default-features = false`** — a
   fact in a dependency's `Cargo.toml`, not a property of anything here. A feature unification elsewhere
   in the graph would turn an unchosen intermediary back on with nothing in this crate changing.
2. **`jarvis-models` already refuses the same thing, for the same reason, in its own client**: "Inherited
   from the environment by default, which would expose prompts and local-model traffic to an unchosen
   intermediary." An MCP session carries tool arguments and results, so the argument applies at least as
   strongly — and the two adapters were making *different* choices about the same hazard.

There is also a shape question. `LoopbackHost` in `jarvis-core` exists so that a local client "cannot be
aimed off loopback" — the host is **unrepresentable** rather than validated, and its own doc says so. The
remote case had no equivalent, so the safe shape existed for one transport and not the other.

## Decision

**1. A remote endpoint is a validated value, not a string.**

`McpHttpEndpoint::parse` enforces:

- the scheme is `http` or `https`;
- **no userinfo** — a credential in a URL is a substring of every log line that mentions the endpoint.
  This project learned that for provider keys (`ApiKey::new` rejects a pasted URL for the same reason);
  the rule is that a secret must be a *distinct value* a redactor can target, not a fragment of another;
- **no `#` fragment**, which is never sent to a server, so a URL carrying one most likely came from a
  browser and expresses something other than what the request will do;
- no interior whitespace or control characters (outer whitespace is trimmed, because a URL pasted from a
  terminal routinely carries a newline and refusing that would be an unhelpful failure);
- a non-empty authority;
- **TLS off loopback.** A plaintext remote MCP session carries tool arguments and results in the clear.
  This is a refusal rather than a warning because the failure of a warning is silent while the failure of
  a refusal is a message.

`connect_http` takes `&McpHttpEndpoint`, so a caller cannot supply a value that violates any of these.
The rules are a property of the value rather than a convention at one call site.

**2. Loopback is recognized the same way `jarvis-models` recognizes it.**

Whole-host comparison against `127.0.0.1`, `localhost`, and `[::1]` — never `starts_with`. A host that
merely *begins* with a loopback string (`127.0.0.1.evil.example`) is remote, which is the case that would
otherwise let an attacker-supplied name bypass the TLS rule. The list is narrow on purpose: reporting
remote when the traffic actually stays local is the safe direction.

**3. The HTTP client is built by this crate and handed to the SDK.**

The SDK exposes `StreamableHttpClientTransport::with_client`, and `reqwest::Client` implements its
client trait, so this crate can state every choice itself:

| Choice | Why it is stated here |
| --- | --- |
| `.no_proxy()` | The SDK does not. Proxy-off is a **side effect** of a feature flag in the SDK's manifest; stating it makes it a property that a test can falsify. |
| `.redirect(Policy::none())` | The SDK does the same, but stating it means the property survives an SDK change. A redirect can move tool arguments and an `Authorization` header to an origin the operator never named. |
| `.https_only(true)` off loopback | Belt to the parser's braces: the *client* refuses a plaintext remote endpoint even if a future caller reaches the connect path another way. |
| `.read_timeout(...)`, never `.timeout(...)` | `P2-008` found by running that a client-level `timeout` bounds the whole response body, so it terminates a healthy open-ended SSE stream at the deadline. The MCP response may be `text/event-stream`, so a total bound is wrong for it. |
| `.connect_timeout(...)` | Bounds the part that genuinely can stall indefinitely on a peer that accepts nothing. |

Declaring `reqwest` directly adds **no package** — it is already in the graph through `rmcp` — so the
only cost is that the choice becomes visible, which is the point.

**4. What this deliberately does NOT claim: DNS and private-range blocking.**

The module records this rather than glossing it. A point-in-time answer is the wrong shape for the
question: a name can resolve to a public address when checked and a private one when connected, which is
exactly why `security.md` lists "DNS rebinding defense" as its own mitigation. What can be made
structural is enforced structurally; the residual is named, and an address-pinning connection is the
next mechanism if a target server justifies one.

## Consequences

- **`connect_http` is now exercised against a real HTTP server** (`tests/http.rs`, 10 tests), closing the
  limit `P3-008f` recorded. The server is hand-written HTTP/1.1 rather than a framework, for the reason
  every other fixture in this project is hand-written: a fixture sharing the code's conveniences can
  agree with it and disagree with the specification. It records what it received, so the assertions are
  about bytes.
- **Two transport-level behaviours are now observable, and both are asserted**: the request carries
  `Accept` for both response media types, `Mcp-Protocol-Version`, and `Mcp-Method`; and a redirect is
  refused **with the redirect target receiving nothing**.
- **The redirect test's assertion order is load-bearing, and falsifying it is what showed why.** Allowing
  redirects made the test fail on a *message* assertion first — the refusal that arrives is a protocol
  error whose text does not mention `302` — so a message check placed before the security check ends the
  run before the property is examined, and the test would fail for a formatting reason while the thing it
  exists to catch went unobserved. The security property is now asserted first: re-running the
  falsification reports the target actually receiving the request (a `GET` carrying
  `mcp-protocol-version` and a `referer` for the original host).
- The `GET`-for-SSE path is **declined explicitly by the test server** rather than left hanging. A peer
  that answered nothing would leave the client waiting and the test would read as a transport bug. This
  also means the standalone-stream behaviour is *not* covered — recorded as a limit rather than implied
  by silence.
- Still **not reachable from `jarvisd`**: nothing reads a `config.toml`, so an endpoint is constructed by
  a caller. `P3-009` owns the daemon that would, and it now has a validated value to load into
  configuration rather than a string to check there.

## Alternatives rejected

- **Keep `&str` and validate inside `connect_http`.** Puts the rules in one function that every future
  caller must route through, and leaves no value a configuration loader can *hold* before connecting.
  The rules would also be unreadable at the call site, which is where an operator's mistake appears.
- **Accept both `http` and `https` and warn on plaintext remote.** A warning's failure mode is silent;
  the operator sees a working session and never learns the traffic is in the clear.
- **Block private/link-local IP ranges by parsing the host.** Wrong answer for the question: it resolves
  a name at one instant and blocks on a different one. Doing it here would read as coverage for the
  rebinding case it cannot address.
- **Use the SDK's `from_uri` convenience constructor.** Fewer lines, and it would silently opt out of
  this project's proxy and TLS policy while looking identical — the "defaults inherited from a
  dependency" problem this ADR exists to remove.
