# ADR-0024: A server does not name itself, and two MCP tool names never become one identifier

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-007` established that MCP revision `2026-07-28` is a stateless rewrite and selected `rmcp` 3.4.0
for the protocol work. Before writing any transport, three decisions have to be settled, because they
are about *authority* rather than about bytes — and each is decided once, for every MCP server that
will ever be configured.

**1. Whose name identifies a server?** An MCP server reports a `name` in `serverInfo`. The
specification says of it: "The server `name` (from `serverInfo`) is not guaranteed to be unique across
servers and **SHOULD NOT** be relied upon for disambiguation." The field is a self-assertion by the
party being identified — the same shape as a JWT's `iss` if it were trusted in place of the URL the
key was fetched from.

**2. How does a tool name become a JARVIS identifier?** Two constraints meet here and they do not
agree. MCP tool names `SHOULD` be 1–128 characters of `A-Za-z0-9_.-`; JARVIS names are **lowercase
ASCII only**, dots are **permitted in the namespace and forbidden in the name**, and each segment is
capped at 48 characters. So an MCP name may legally be something JARVIS cannot hold.

The obvious translations are all lossy, and each one loses the same thing: the ability to tell two
tools apart. Lowercasing maps `getUser` and `getuser` together. Stripping the dot — which the
specification itself suggests, "prefixing tool names with a server identifier" — maps
`admin.tools.list` and `admin.tools` together under a prefix. Truncating maps `aaaa…x` and `aaaa…y`
together while looking like it succeeded.

**3. What does a server-supplied schema authorize?** Not risk, effects, or scopes — those are JARVIS's
to declare, and `docs/research/integrations/mcp.md` records the mapping. But the schema is not inert:
the `x-mcp-header` annotation makes the **client** put an argument into an HTTP header, and the
specification puts the validation duty on the client, requiring that an invalid annotation exclude
**that tool** from `tools/list`.

## Decision

**1. The operator chooses the server name; the server's own claim is evidence.**
`ServerName` is validated as narrowly as a tool-name segment — lowercase ASCII, digits, `-`, `_` — and
dots are refused even though a namespace may contain them, because `mcp.a.b` would otherwise be
ambiguous between "server `a.b`" and "server `a`". `ReportedIdentity` holds what the server said about
itself, bounded and kept separate. It is never an identifier, and `agrees_with` exists so a server
changing its self-assertion between refreshes is *visible* rather than silently absorbed.

**2. Names that are not representable are refused or digested, never rewritten.**
`canonical_tool_name` uses the server's name unchanged when a JARVIS name segment can hold it exactly,
and otherwise hashes it — SHA-256 over the whole name with a length prefix, rendered as `h` plus eight
hex digits. Truncation is refused explicitly. `Prefixed` and `Bare` differ only in namespace, so a
name they cannot represent is a refusal rather than a silent digression into hashing.

The two strategies produce:

| Strategy | `list_issues` on server `github` | Collides across servers |
| --- | --- | --- |
| `Prefixed` | `mcp.github.list_issues` | no |
| `Bare` | `mcp.list_issues` | **yes**, deliberately |
| `Hashed` | `mcp.github.h1a2b3c4d` | no |

**3. Every translation is checked against what is already assigned, and the check is keyed on
`(server, tool)`.**
`NameAssignments::assign` takes both. That pairing is the whole subtlety: **the same tool name from
the same server** is a `tools/list` refresh and must be accepted, while **the same tool name from a
different server** is exactly the collision `Bare` causes and must be refused. Keying on the tool name
alone conflates them — which is what the first version of this code did, and two of its own tests
found it.

**4. A `$ref` in a server-supplied schema is never resolved.**
`jarvis-tools` already refuses a remote reference. This revision loosened schemas to allow any
2020-12 keyword, so the refusal is *more* load-bearing than before, not less: resolving one would mean
fetching a URL a remote server chose.

**5. `x-mcp-header` is validated against all four protocol constraints, and a failing tool is excluded
by itself.** Primitives only (`number` is explicitly not permitted, and an `integer` bound must sit
inside the JS-safe range), reachable only through `properties` keys, unique case-insensitively, and a
valid HTTP field name. `check_tool_schema` returns a **report**, not a `Result`, because both outcomes
are ordinary — and a rejected tool reports **no** mirrored headers, so a caller cannot act on a partial
set.

## Consequences

- The security-relevant rules are **pure functions with no transport**, so they are testable without
  standing up a peer, and the transport can be reimplemented or the SDK replaced without revisiting
  them. This is why the crate does not depend on `rmcp`: expressing these rules against the SDK's
  types would couple the authority boundary to a third-party data model.
- The reachability rule is the one that looks like arbitrary strictness and is not: the specification
  defines header extraction as reading the value at "the exact property path", and that definition
  only has one answer when the path is unique. Under `oneOf` there are two; under `items` the path
  needs an array index a header cannot carry. Skipping the check would mean not knowing which value to
  send.
- **The name-half derivation is injective within one server.** Distinct remote names never share an
  identifier under any strategy, which is asserted rather than assumed — a lossy step introduced later
  fails that test. The collision check's real job is therefore the cross-server case, and the `Bare`
  strategy's documented risk is what the check exists to make safe.
- A hashed name is **recognisable as one** (`h` prefix) and stable, so a re-`tools/list` maps the same
  remote tool to the same identifier. Without that, a refresh would orphan every stored call binding.
- `CanonicalToolName` keeps the **server's** name alongside the canonical id, because `tools/call` must
  send the server its own name back. Sending the canonical identifier would be the translation applied
  twice, and that bug is invisible until a real server rejects a call.

## Limits, stated rather than implied

- **Nothing here has been exercised against a real MCP server.** These are pure functions tested as
  pure functions. "The translation is correct" and "a real server's tool list translates" are different
  claims and only the first is proven. `P3-008` owns the second.
- **The SDK is not a dependency yet.** This is deliberate — the rules are about JARVIS identifiers — but
  it means nothing here has been checked against a real `tools/list` payload, and the field names used
  (`serverInfo`, `x-mcp-header`, `inputSchema`) are taken from the recorded specification rather than
  from bytes.
- **The hash is a disambiguator, not a security boundary.** Eight hex characters; an attacker who can
  make prefixes collide can grind a colliding digest. The authority comes from the canonical identifier
  being stored, not from the hash.
- **No operator surface exists for the naming strategy.** It is an enum with a `serde` impl and no
  configuration key, so a `Bare` strategy cannot currently be selected — and when it can be, a
  collision will refuse the *second* server's tool at translation time rather than at configuration
  time, which is later than an operator would want to learn it.
- **`ReportedIdentity` disagreement is recorded but not acted on.** `agrees_with` exists and has a test;
  nothing calls it, because the host adapter that would is `P3-008`.
- **The conformance check does not validate the schema against JSON Schema.** It checks the annotation
  rules and hands the document to `jarvis-tools`, which does that separately. A schema with no
  annotation is passed through with no structural opinion at all, which is correct for this layer and
  worth not mistaking for validation.

## Alternatives considered and rejected

- **Trust the server's reported name as the namespace.** Rejected: it relies on the one field the
  specification warns is neither unique nor reliable, and it lets a server choose its own namespace —
  so a hostile server could claim `jarvis.files` and have its tools classified as native. The
  namespace prefix is what marks a definition third-party, so it cannot be server-chosen.
- **Lowercase the tool name into a valid identifier.** Rejected: a lossy step that maps `getUser` and
  `getuser` onto one identifier.
- **Strip or replace dots, as the specification's prefixing suggestion is often read.** Rejected: it
  maps `admin.tools.list` onto `admin.tools`, so a call for one tool would run the other — with the
  other's declared risk, effects, and scopes.
- **Truncate an over-long name.** Rejected, and refused explicitly in code: a shortened name *is* a
  different name and can collide with a real one. A digest cannot collide with a real name because it
  is obviously not one.
- **Refuse any name that is not already lowercase-safe.** Rejected: `getUser` is a valid MCP name and
  refusing it means the tool is unreachable through `Hashed`, which exists precisely to make an
  unrepresentable name usable without pretending it is representable.
- **Make the naming strategy configurable only.** Rejected: the strategies are a `serde` enum, but the
  translation function takes them explicitly, so the unit under test is the rule rather than a lookup.
- **Have `NameAssignments` key on the tool name alone.** Rejected — and this was the implemented
  version, found by its own tests: it silently permitted two servers to share an identifier and refused
  a legitimate refresh. The keys are `(server, tool)`.
- **Return early on the first conformance problem.** Rejected: an operator fixing a schema would need
  one round trip per problem. Every check runs, problems are bounded to eight, and the total is
  reported so a reader is not misled into thinking they saw them all.
- **Fail the whole `tools/list` for one bad tool.** Rejected: the specification requires excluding the
  tool, and the stated reason is the right one — "a single malformed tool definition does not prevent
  other valid tools from being used".
- **Take the MCP SDK as a dependency for its types.** Rejected: it would put the naming rules on the
  SDK's data model, so a major version bump could change a security-relevant decision.

## Revisit if

- `P3-008` finds that a real server's `serverInfo` or `tools/list` payload uses a field name or shape
  this crate assumed from the specification rather than from bytes.
- An operator needs a naming strategy that is neither prefixed nor hashed — for instance prefixing with
  a *different* label than the server name.
- A server legitimately offers more than one tool whose names hash to the same eight hex characters,
  at which point the digest length becomes a measured problem rather than a theoretical one.
- The `Bare` strategy is exposed in configuration, at which point collision refusal should move to
  configuration validation so an operator learns before first use.
