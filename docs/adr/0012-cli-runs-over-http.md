# ADR-0012: The CLI Reaches Runs Over HTTP, With A Loopback-Only Endpoint Type

- Status: Accepted
- Date: 2026-09-22

## Context

`P2-008` adds `jarvis ask` (and `chat`, which is deferred). Two questions it raises are durable
architecture rather than coding detail, and neither is answered by the existing records.

**1. Which transport does the CLI use for runs?** ADR-0011 makes HTTP a first-class peer transport
and records local IPC as the **preferred** transport for the CLI and desktop. That reads as though the
CLI should reach runs over its pipe or socket. But protocol v1 serves `status` and `health` only:
there is no `start_run` command, no event stream, and no run-event DTO on the local protocol. The run
surface exists solely on `/api/v1`. So the CLI's two existing commands use local IPC while its new one
must use HTTP, and "preferred" does not by itself say whether that is right or a stopgap.

**2. How does a client know which host to send the profile credential to?** The credential is a
bearing bearer token that admits a caller to the daemon. The HTTP transport is loopback-bound by
default and reaching another interface is remote mode (`P10-004`). If the client derives its target
from configuration as a free-form string, then a single bad config value, a copy-paste from a remote
deployment, or a future helper that resolves a hostname would send the credential somewhere the
daemon never intended — and nothing in the type system would object.

## Decision

### Runs are reached over HTTP; local IPC stays the transport for `status` and `health`

`jarvis ask` speaks `/api/v1` because that is where runs are defined, and this follows ADR-0011 rather
than working around it: HTTP is a peer transport, and a client uses the transport that carries the
operation it needs. Adding a second run API to protocol v1 was rejected — it would duplicate the run
contract, and two write paths to one state machine is how the weaker path becomes the way in.

The transport is `reqwest` 0.13.5 with `rustls`, already resolved in this workspace through the model
adapter, configured with the same hardening that record chose:

- **No client-level total timeout.** This is the one decision ADR-0011's sibling record does not cover,
  and it was learned by running the client: a client-level `reqwest` timeout bounds the *whole response
  body*, so on a client that also serves an open-ended SSE stream it terminates every healthy stream at
  the deadline. Non-streaming requests set a per-request timeout; only `read_timeout` bounds a stream,
  because a stream has no defined total length.
- **Proxies disabled.** `reqwest` reads `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY` from the
  environment by default, which would route a credentialed request through an unchosen intermediary.
- **Redirects not followed.** A redirect can move an `Authorization` header to another origin.
- **Certificate verification stays on.** No insecure path is offered.
- The credential is sent **only** in `Authorization: Bearer`, never in a query parameter, where it
  would become a substring of every access log the request passed through.

### The loopback target is unrepresentable, not validated

`jarvis_core::LoopbackHost` holds **a port only**. It has no address field, so no constructor, argument,
or configuration value can aim a request at a non-loopback host, and there is no check a later code
path could skip. The address is derived from a constant.

This is the same principle as returning a credential-free gRPC endpoint string elsewhere in this
project: making a mistake structurally impossible beats documenting that it is discouraged. A type with
a validated `host: String` would be one deref away from leaking the credential, and its safety would
depend on every future caller remembering a rule.

### Unsafe path segments are refused, not escaped

A run identifier that the client interpolates into a request path is validated against a strict
character set before it is used. A value containing `/`, `?`, `#`, or a percent-escape would otherwise
change the request's target — turning a read of one run into a different route, or appending a query
parameter. The value normally arrives from the daemon's own reply, so a rejection means a broken or
hostile response, and refusing is the honest outcome.

### The SSE decoder is written here, not adopted

`docs/research/integrations/rust-http-client-and-sse.md` already recorded this decision for the model
adapter and it applies unchanged: `eventsource-stream` is a `nom`-based transformer, and adopting it
would add a parser dependency for the narrow subset the daemon emits. The decoder buffers **bytes** and
splits on separators, so a multi-byte character split across a TCP chunk survives, and it compares a
frame's declared `event:` name against the parsed payload's kind, so a one-sided rename is a refusal
rather than an event silently typed as something else.

## Consequences

- The CLI now has two transports, and each operation uses the one that carries it. `status` and
  `health` need no configuration; `ask` needs the HTTP transport enabled, and it says so with the exact
  config keys rather than reporting a connection failure.
- The HTTP transport is off by default, so `jarvis ask` is not usable on a default install until the
  operator enables it. This is deliberate: ADR-0011 made a listening port separately enabled because it
  is a larger surface than an OS-protected pipe, and a client that silently required a port would
  undercut that.
- Two exit statuses were added (`9` cancelled, `10` run failed) so a script cannot read a cancelled or
  failed run as success. A cancelled run is settled, not successful, and collapsing the two is how a
  scheduler records a stopped task as complete.
- Runs still do not settle, because nothing invokes a model until `P2-009`. `jarvis ask` therefore
  waits on a stream that has nothing further to send, and this is recorded rather than hidden.
- The run-event wire shape and the SSE framing are now documented in `docs/api/contracts.md` as
  implemented, so a client author has one description rather than having to read the daemon.

## Alternatives

- **Add runs to local protocol v2:** rejected. It duplicates the run contract in a second transport, and
  two write paths to one state machine is how the weaker path becomes the trusted one. If runs are later
  wanted on local IPC, that is a deliberate v2 with the DTOs reused, not a parallel implementation.
- **Make HTTP the CLI's only transport:** rejected. It would give up the OS-protected transport and the
  machine-global namespace problem already solved, which ADR-0002 records as the preference for local
  clients.
- **A free-form `--endpoint` flag or an endpoint URL in configuration:** rejected. It turns the
  credential's destination into operator input with no structural guard.
- **Reuse the model adapter's transport type:** rejected. It is a port for provider JSON with its own
  request shape, and the daemon's API is a different contract; sharing the type would couple two
  unrelated dependencies and make a change for one affect the other.
- **A client-level timeout with a longer value:** rejected. The defect is categorical, not a matter of
  degree: any total timeout kills a healthy stream eventually.

## Revisit When

- Runs are added to the local protocol. That is a v2 decision with a compatibility window, and it should
  reuse the REST DTOs rather than define a third shape.
- Remote mode (`P10-004`) is implemented. It is the first case where the target is legitimately not
  loopback, so `LoopbackHost` must not be widened by a flag; TLS termination, certificate validation,
  and a distinct credential scoping decision belong to a new ADR.
- A third client needs runs, which would make a generated client from one schema worth the cost.
- `axum` 0.9 is adopted, since it changes `serve` behavior and the SSE writer this client parses.
