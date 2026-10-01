# ADR-0121: A provider's credential is a file path, and a partial provider is refused rather than guessed

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** live model provider composition — making the `openai-compatible` adapter reachable from a
  running daemon, so a configured profile answers a run with a real model instead of the deterministic
  scripted one.
- **Relates to:** `ADR-0119` (the model may request a tool; policy decides), `ADR-0120` (a decided approval
  continues the run), `ADR-0031` (a dependency's permissive default is not a policy), `ADR-0038` (a network
  request is never a local caller), `ADR-0091` (a value redacted in one place and printed in another). The
  integration contract is `docs/research/integrations/openai-compatible-model-api.md`; the request surface is
  `crates/jarvis-models/src/openai/`.

## Context

The OpenAI-compatible adapter existed and was tested against offline fixtures, but nothing in a running
daemon could select it. `executor_model` named a model and the composition root built the scripted gateway
for the one name it knew; the live adapter was reachable only from its own tests. So a user could install
JARVIS, start the daemon, hold a conversation, and get a deterministic scripted answer — with no
configuration key that changed that.

Composing the live adapter means the daemon must carry three provider coordinates (a base URL, a model
name, and a credential) from configuration into a constructor, in a codebase whose whole posture is that
credentials do not enter configuration, logs, or errors. Two constraints shaped the decision.

**Configuration is a document that gets printed.** `Config::to_toml` round-trips a profile so an operator
can inspect the resolved state, and `Debug` on the config is derived. A field holding a real key would be
written to disk by the same code path that writes every other setting, and printed by the same `{:?}` that
prints every other structure. A credential in configuration is therefore not merely discouraged — it is a
value that a *routine* operation republishes.

**A name is not an implementation.** `executor_model` is an operator-facing string. The previous code
compared it against a constant and otherwise errored, which means the mapping from "what an operator
wrote" to "which `dyn ModelGateway` exists" lived in a `match`. Adding a second implementation makes that
`match` a dispatch table, and a dispatch table with no rule for "these coordinates do not belong to that
name" will happily build a live adapter for a scripted name and vice versa.

## Decision

**1. Configuration carries a path to a credential file, never the credential.**

`executor_api_key_ref` is an `Option<PathBuf>`. The key is read **once**, at composition, and moved into the
adapter's `ApiKey`. It never becomes a field of `DaemonConfig`, so it cannot be re-serialized by
`to_toml`, cannot be printed by the derived `Debug`, and cannot be written to the SQLite profile row. The
document a user inspects and the document a user commits are both key-free.

**2. The path must be absolute, and that is validated rather than documented.**

A relative path resolves against the daemon's working directory, which for a service is not a directory a
human chose and can differ between the process that validates the config and the process that reads the
file. `validate` refuses a relative `executor_api_key_ref` at load time, so "which file?" is answered once,
where the operator can see the error, instead of whenever `fs::read_to_string` happens to run.

**3. A partial provider is refused, not completed from defaults.**

`validate` raises `IncompleteModelProvider` when the live implementation is selected but any coordinate is
missing, and `ModelProviderWithoutImplementation` when coordinates are present but the implementation is
not selected. Both are refusals rather than fallbacks:

- A **partial** provider is the dangerous shape. A base URL with no model name looks configured, starts
  cleanly if the fields are each independently optional, and fails on the first run with an error from the
  provider rather than from JARVIS — sending the operator to debug a vendor account for a missing local
  line.
- **Coordinates without an implementation** are the opposite error: a settings block that has no effect.
  Silently ignoring it means an operator who configured a provider believes their setting is in use while
  the scripted gateway answers every run.

Each field being `Option<String>` is what makes this expressible at all; the validation is what makes the
combination legal only when it is complete. **The default is not "fill in a blank" but "refuse to start",
because a daemon that starts with a half-configured provider is a daemon whose answers cannot be trusted**
(`ADR-0031`).

**4. The name selects the implementation through one constant, and unknown names are refused.**

`LIVE_PROVIDER_MODEL_NAME` (`"openai-compatible"`) is the single spelling that selects the adapter,
exported from `jarvis-storage` so configuration validation and the composition root cannot disagree about
it. `Executor::build` returns `UnknownModel` for anything else, so an operator who writes a vendor name
(`"gpt-4o"`) as the executor model gets a startup error naming the value rather than a silent fallback to
the scripted gateway. **The model *id* is a separate setting on purpose** — `executor_model_name` is the id
sent to the provider, while the executor name chooses the transport — and conflating them would make the
transport a function of which model a user happens to want.

**5. The credential is validated as a value, not merely carried.**

`Executor::build` constructs the adapter's own types (`ApiKey`, base URL) rather than forwarding strings,
so an empty key, a whitespace-bearing key, or a key that is actually a pasted URL is refused at startup
with `InvalidProviderField` naming the *field*. The error carries the field name and **not the value**,
because an error message about a credential is a place a credential gets printed. `Executor`'s `Debug` is
hand-written to redact the credential, so `{:?}` on the composed executor is safe by construction rather
than by discipline.

## Consequences

- **A configured profile now answers with a real model.** The whole point: the adapter that had only
  fixture coverage is reachable from `jarvisd`, and `compose_executor` is the one place the mapping from
  configuration to a gateway exists.
- **The config document stays safe to print and commit.** No key is ever a field; the operator's file
  contains a path they chose. This is the same posture as the storage DSN, which also carries a reference
  rather than a credential.
- **Startup is where provider faults surface.** A missing file, an empty file, a malformed base URL, and an
  unknown executor name all stop the daemon before a client can connect, rather than failing the first run
  after it was accepted.
- **The credential is read once and never refreshed.** A key rotated on disk while the daemon runs is not
  picked up; a restart is required. Recorded as a limit rather than solved, because key rotation needs a
  reload path and a decision about whether replacing a credential replaces the executor.
- **No keyring integration.** The path is a file path; an OS keystore integration would be a different
  resolver behind the same field, and no requirement has asked for it yet.
- **One provider, one credential.** There is no per-model or per-session routing; a second provider means a
  second decision about where its coordinates live.

## Alternatives

- **Put the key in the configuration document.** Rejected: `to_toml` and derived `Debug` mean a routine
  round-trip republishes it, and a committed config would contain a live credential — a secret in a file
  whose entire purpose is to be inspected and shared.
- **Read the key from an environment variable directly in the composition root.** Rejected for *this*
  field: `JARVIS_EXECUTOR_API_KEY_REF` already lets an operator name a path from the environment, so the
  indirection is available without making an environment variable a supported way to hold a long-lived
  secret. Environment variables leak into process listings and crash dumps.
- **Treat a partial provider as "not configured" and use the scripted gateway.** Rejected: it converts a
  configuration mistake into apparently-working software whose answers come from somewhere the operator did
  not choose. Refusing to start is the honest answer.
- **Ignore coordinates when the implementation is not selected.** Rejected: it makes a settings block with
  no effect, which is exactly the "accepted and silently ignored" shape this codebase treats as a defect.
- **Let `executor_model` name a vendor model and infer the transport.** Rejected: it makes the transport a
  function of a string with no schema, and it removes the one value that says which implementation an
  operator selected.

## Conditions that would justify revisiting

- A requirement for **credential rotation without a restart**, which needs a reload path and a decision
  about whether a new credential replaces the composed executor or mutates it.
- An **OS keystore or secret-manager integration**, which would put a resolver behind
  `executor_api_key_ref` (or a sibling field) and is a new trust boundary worth its own record.
- **A second live provider**, which would make the name-to-implementation mapping a real dispatch table and
  raise the question of whether provider coordinates belong in their own document rather than flat on the
  daemon.
- A requirement for **per-session or per-model provider selection**, which conflicts with a single
  composed executor and would move selection from startup into the run.
