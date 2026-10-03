# ADR-0127: The naming strategy is the document's, not the caller's

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P3-008k`

## Context

`NamingStrategy` decides how a server-supplied tool name becomes a canonical JARVIS identifier. It has three
values — `Prefixed` (`server.tool`), `Bare` (`tool` alone), and `Hashed` (`server.digest`) — and it was a
**parameter** to `McpHostConfig::connect`, so the caller chose it.

The daemon chose `Prefixed` and hardcoded it, with a comment that recorded the situation honestly:

> `Bare` exists for an operator who wants short names and accepts that two servers cannot both offer one, which
> is a choice a configuration file would state — and does not yet, so the safe value is the only one used.

Three consequences followed, and each is a defect rather than a limitation:

1. **An operator could not express the choice.** `Bare` was reachable from a test and from nothing an operator
   writes, so the surface was narrower than the vocabulary it exposed.
2. **A caller could disagree with the document.** Anybody constructing a host could pass a strategy the
   configuration file did not state, so two builders of the same configuration could produce two different tool
   sets — the property `dispatch.rs` refuses for adapters and the catalog refuses for duplicate names.
3. **A collision was unreachable through the daemon's own path.** `Prefixed` cannot collide, so
   `HostError::Collision` could not be produced by `compose_host`; the daemon's test for it asserted the
   hardcoding instead (`the_daemons_naming_strategy_is_the_one_that_avoids_collisions`), which is a test of a
   constant.

## Decision

**`[naming] strategy = "<name>"` is a section of `mcp-servers.toml`, and `McpHostConfig::connect` takes no
strategy.**

`McpHostConfig` owns the value, and `naming_strategy()` reads it back. `connect` reads its own field, so a
caller cannot pass a strategy the document disagrees with — the disagreement is unrepresentable rather than
merely refused.

**An absent section means `Prefixed`.** The default is stated on `StrategyName`, the configuration's own type,
rather than derived onto `NamingStrategy` — a `#[serde(default)]` on the domain enum would be *that type
choosing a security-relevant default for one consumer*. The default belongs where the consequence is, and the
consequence is which tools a model is offered. `Prefixed` fails towards serving: a collision is the failure the
namespacing prevents.

**`StrategyName` is written out rather than derived from `NamingStrategy`.** The domain enum already has
`Deserialize`, and reusing it would have been shorter. It is restated for the reason every vocabulary type in
this repository is restated: the domain's `as_str` is its wire form, and a rename there must not silently change
what a configuration document means. The mapping is exhaustive, so a fourth strategy cannot be added without a
document name for it.

**The collision path is now reachable from configuration, and the daemon's test asserts both halves.** Writing
`strategy = "bare"` with two servers offering one name is a **refusal**; the same servers with no naming section
**compose**. A test of the refusal alone would pass if every document composed.

## Consequences

- **`connect` lost an argument and eight call sites were updated** — six in `jarvis-mcp-transport`'s tests and
  two in `host_config.rs`'s own. The `a_cross_server_collision_refuses_the_host` test now writes the strategy
  into its document instead of passing it, which is what makes it a test of the document.
- **`McpHostConfig::none()` is `const` and names `Prefixed` explicitly.** It has no document to read, so it
  cannot consult a section; naming the safe value directly — with the reason — is clearer than routing it
  through `StrategyName::default`, which is the default *for an absent section*.
- **The daemon's own test changed from asserting a constant to asserting a read.** The old one passed a document
  and checked that two unreachable servers composed; a daemon that ignored the section passed it. The new pair —
  `the_daemon_reads_the_configured_naming_strategy` and `a_configured_bare_strategy_can_collide_and_is_then_
  refused` — fails if the section is ignored, in the transport layer and in the daemon.
- **Both directions are falsified.** Replacing the parsed strategy with `Prefixed` in `parse` fails the
  transport's collision test **and** the daemon's read test; making `Bare` the default fails
  `a_configured_stdio_server_becomes_a_callable_tool` — the tool identifiers lose the prefix, `mcp.search`
  instead of `mcp.local.search` — and the absent-section assertion in the collision test.
- **Recorded as a limit:** `Hashed` is now expressible but **not exercised against a real server**, because no
  fixture reports a name that cannot be represented. The strategy is reachable and the mapping is asserted;
  what is absent is a server whose names require it.
- **Recorded as a limit:** the strategy is **per host, not per server**. A setup where one server's names need
  hashing and another's are fine has to use `Hashed` for both, or be split across two daemon profiles. The
  section shape is where a per-server override would go.
- **Unchanged:** no connection pooling or reconnect, and `seen_before` remains empty, so an identity drift across
  a restart is still invisible. Both are recorded in `P3-008`'s and `P3-009`'s entries.
