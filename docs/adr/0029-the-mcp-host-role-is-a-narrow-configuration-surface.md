# ADR-0029: The MCP host role — a narrow configuration surface, and one parser per document

**Status:** Accepted

**Date:** 2026-09-23

## Context

Nine MCP slices (`P3-008a`..`P3-008h`) recorded the same limit in different words: **"a capability a
caller can use, not one an operator can reach"**. The pure crate could translate a listing nobody had
fetched; the transport could fetch one nobody had translated; the adapter could run a call nothing
dispatched. Every piece was tested and **nothing read a configuration file**, so the entire integration
was reachable only from a test.

Two facts made this the last piece rather than an early one. It is the only place where all three halves
are visible at once — which in this project is where defects have been found with monotonous regularity
(`P3-006a`, `P3-006b`, `P2-009b`, `P3-008f`). And `McpCatalog::has_cross_server_collision()` had existed
since `P3-008c` with a documented instruction — "a caller should refuse to serve when this is true" — and
**no caller to do it**. A guard with no caller is a claim, not a control.

## Decision

**1. The configuration lives in `jarvis-mcp-transport`, and it owns only the MCP section.**

The daemon's own schema stays in `jarvis-storage`. This crate parses the `[mcp]` section and nothing else,
so there is **one schema owner per document** and no overlap for two validators to disagree about. The
daemon will read its own file, hand this crate the section, and get back a connected host — a shape that
keeps the daemon's configuration schema from growing an MCP vocabulary it has no reason to hold.

> **CORRECTION (2026-09-23, `P3-008j`): the `[mcp]`-section half of this decision was rejected, and the
> reasoning still holds.** `apps/jarvisd` reads `mcp-servers.toml` as a **whole document** and calls
> `McpHostConfig::parse` directly. An `[mcp]` section inside `config.toml` was rejected for the reason this
> decision already gives from the other side: the daemon's schema is validated by **key allowlist**, so an
> `[mcp]` section would make `jarvis-storage` learn one protocol's configuration vocabulary — the dependency
> this paragraph was trying to avoid, arriving through the parser rather than through a type. A separate
> document also keeps an MCP failure from being a daemon startup failure, which is the same reasoning that
> makes one failed `tools/list` an exclusion rather than a failed run. **`parse_host_section` was a public
> helper added for the rejected shape and it had no caller outside its own test**, so it was removed in
> `P3-008j` — see the note where it used to be in `host_config.rs`. What survives from this decision is the
> part that mattered: this crate owns the host schema and `jarvis-storage` owns the daemon's, with no
> overlap.

**2. The posture vocabulary is two classes, and the default is the severe one.**

An operator writes `class = "read-only"` or nothing. Nothing means
[`ToolEffectPolicy::unclassified`]: risk 3, every call held.

That narrowness is the decision. A full policy needs effects, risk, scopes, retry, idempotency, and
sensitivity, and a configuration file offering all six would be a vocabulary whose entries nothing
consumes — the naming-scheme-with-no-users problem `DEFAULT_MCP_SCOPE` already records. More importantly
**the safe posture already exists and is severe**, so the failure of leaving something out is a refusal
rather than a permission. The one thing an operator can usefully narrow today is "this only reads", because
that is the common case and the difference is stark.

An unknown class is **refused**, never defaulted. A typo must not leave a server at the permissive value an
operator was trying to set — the failure direction is what makes this worth its own test.

**3. A collision refuses the whole host, and the refusal carries the catalog's own description.**

This is the first caller of the collision check. Serving a catalog whose contents depended on
configuration order would make the reachable tool set a function of an ordering nobody declared
meaningful.

**The message was wrong at first, and the reason is a class of defect this project keeps finding.** The
error is built from `CatalogExclusion`'s `server` field, but a collision entry records only the server that
**lost** the assignment — so two colliding tools produced `["bravo", "bravo"]`, naming one server twice
while the remedy ("rename one of them") needs **both**. The catalog's `NameCollision` reason text names
both sides, so that is what the refusal carries now. A refusal whose text does not identify what must
change is the opaque-diagnostic defect, arriving in the one place an operator has nothing else to go on.

**4. One connection per server, shared between the catalog build and the adapters.**

`HostedServer` holds an `Arc<McpConnection>`. Two connections to one server would be two sessions — or two
child processes — whose lifecycles diverge, and shutdown would close only one. For the same reason,
`McpHost::close` **drops the adapters before closing**, because a connection can only be closed by its last
owner; a connection still shared at that point is **reported** rather than claimed as stopped.

**5. A document that cannot be read says where, and claims nothing more.**

The first version of `InvalidDocument` said "an unknown key is refused rather than ignored". That is **a
cause the message cannot know**: a syntax error, a misspelled key, and a wrong value type all arrive as the
same deserialization failure. It was found by a fixture whose Windows path used a TOML *basic* string, where
`\U` begins an invalid escape — a syntax error reported as a key mistake, sending an operator to check keys
that were correct.

The message now carries a **character offset** and a warning about the most likely cause. The parser's own
text is deliberately not forwarded: it can echo the document, and a configuration file holds paths and
hostnames that do not belong in a log line. An offset is bounded, actionable, and reveals nothing.

**6. No credential can appear in this document.**

A server entry names a program or a URL. `McpHttpEndpoint` refuses a URL with embedded userinfo, so a
configuration file cannot hold a secret even accidentally. A remote server needing authentication is its
own slice and belongs in the credential store.

## Consequences

- **The limit is closed at its last step.** A test starts from TOML text and ends with a call: configuration
  → spawn → negotiate → list → translate → catalog → adapter → `Confirmed`. Nine slices of "nothing reads a
  config file" now have a caller.
- **`has_cross_server_collision()` has a caller**, and the `Prefixed`-strategy control in the same test
  proves the refusal is about the collision rather than about the configuration being unusable.
- **Two fixture shapes were wrong before the right one, and both are recorded in the test.** TOML forbids
  re-opening a table after an inline-table value; and a Windows path in a basic string is invalid TOML. The
  second is the one that produced the misleading error message, so a fixture mistake became a **diagnostic**
  finding — the same way `P3-008f`'s wrong task fixture found the untagged-union behaviour.
- **`HostedServer` changed shape** (`McpConnection` → `Arc<McpConnection>`), which is not a small refactor:
  it is the statement that the connection is shared, and it moved ~13 test construction sites. That is the
  cost of saying it in the type rather than in a comment, and it is the right trade here because the
  alternative — two sessions whose lifecycles diverge — fails silently at shutdown.
- Still **not reachable from `jarvisd`** at the time of this slice: the daemon did not yet read an MCP
  document, so this was a configuration surface a caller could drive and not one the daemon loads.
  **CORRECTION (2026-09-23): `P3-008j` closed that.** The daemon now reads `mcp-servers.toml`, connects the
  host, registers its adapters, and dispatches to them, so the shape this decision said was "ready for that
  one call" is the shape that call was made against — with the one correction above.

## Alternatives rejected

- **Parse the whole daemon configuration here.** Two validators for one document, and the MCP crate would
  own keys it has no opinion about.
- **Offer the full `ToolEffectPolicy` in configuration.** A vocabulary with no consumer, in the one place
  where getting an effect set wrong is an unaudited outward action.
- **Default an unknown posture to `unclassified`.** Sounds safe and is not: it makes a typo silently narrow
  an operator's *intent*, and the operator would have no way to tell "I classified this read-only" from "my
  classification was ignored". Refusing is the only outcome that keeps the file meaningful.
- **Report a collision by dropping one side.** Makes the reachable tool set depend on the order servers
  appear in the file — the reasoning ADR for `McpCatalog` already rejects one level down.
- **Forward `toml`'s error text.** It can echo the document, and the document holds paths and hostnames.
- **Open a second connection per adapter.** Two sessions to one server, with shutdown closing one.
