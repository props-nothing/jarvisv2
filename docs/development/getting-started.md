# Development Getting Started

## Current State

The repository contains the architecture baseline, the Rust workspace from `P1-001`, and a Python reference prototype.

Phase 1 (`P1-001` through `P1-012`) has delivered a runnable local foundation:

- `jarvisd` daemon lifecycle, singleton lock, graceful shutdown, readiness, and liveness
- OS-native config/data/cache/runtime/log paths with restrictive permissions
- versioned configuration (schema v1) with migrations, unknown-key errors, and atomic writes
- SQLite schema with migrations, schema-version checks, and a verified pre-migration backup
- local control protocol v1 over a Unix domain socket or a Windows named pipe, authenticated with a profile-bound credential
- structured logs: one JSON object per line, bounded and secret-redacted, plus human console output from the same event
- `jarvis status`, `jarvis health`, `jarvis logs`, `jarvis doctor`, and `jarvis service` with human and `--json` output
- `jarvis ask`, which starts a run through the daemon's loopback HTTP API and renders its event stream
- `jarvis chat`, which holds a multi-turn conversation: one run per turn, replaying the session's transcript
- offline `doctor` diagnosis with stable finding codes, safe evidence, specific remediation, and verified repair
- portable foreground mode where one explicit root holds every managed file
- per-user service planning and drift detection (no service is installed yet)
- an automated process-level acceptance gate in `tests/e2e`, run on Windows, macOS, and Linux CI

Not implemented yet: service installation, log rotation, workflows, voice, and the desktop client. Models, tools,
memory, MCP, and a first connector contract exist; see [TODO.md](../../TODO.md) for exactly which slices are done.

The next implementation task is the first unchecked item in [TODO.md](../../TODO.md).

## Run It Locally

### First run: `jarvis init`, `jarvis start`

```text
jarvis init      # finds your local Ollama, asks which model and which folder, writes the configuration
jarvis start     # starts the daemon in the background and waits until it answers
jarvis chat
```

`init` lists the models your Ollama has and refuses a model it does not list. It writes `model.key` (a placeholder
Ollama ignores) and a validated `config.toml`, and it **never overwrites** an existing configuration without
`--force`. It grants **no folder unless you name one** (`--workspace DIR`, repeatable, or the prompt). For another
OpenAI-compatible server: `jarvis init --base-url URL --model NAME --api-key-file C:/path/to/key.txt` (the key stays in
your file; it never passes through the command). Add `--code-image node:22-alpine --code-interpreter "node -e"` to give
the assistant a no-network throwaway container for code, and `--trust-code` to let it run without asking each time
(`ADR-0133`); web pages already run without asking, and anything with an external effect is still asked. `start` returns once the daemon listens, or says why it exited.
`--root DIR` (an existing directory) keeps a throwaway profile. The manual route below stays available for operators.

In one terminal:

```powershell
cargo run -p jarvisd
```

In another:

```powershell
cargo run -p jarvis-cli -- status
cargo run -p jarvis-cli -- health --json
cargo run -p jarvis-cli -- ask "summarise my inbox"
cargo run -p jarvis-cli -- chat
cargo run -p jarvis-cli -- logs --lines 20
cargo run -p jarvis-cli -- doctor
cargo run -p jarvis-cli -- service
```

`jarvis status` and `jarvis health` connect to the same profile the daemon owns. `jarvis logs`, `jarvis doctor`, and `jarvis service` all work without the daemon, which is the point: they must diagnose a broken installation rather than depend on it.

### Asking (HTTP transport)

`jarvis ask` is the one command that needs the daemon's **HTTP** API, because runs are not on the local
control protocol. ADR-0011 makes HTTP a peer transport (`P2-008`), and the CLI speaks it for runs while
`status`/`health` stay on local IPC.

The HTTP transport is off by default, so enable it before asking:

```toml
# <config>/config.toml
[daemon]
http_enabled = true
http_port = 8765
```

or set `JARVIS_HTTP_ENABLED=1` and restart `jarvisd`. With it disabled, `jarvis ask` explains exactly
that and exits `3` rather than reporting a connection failure.

```powershell
jarvis ask "summarise my inbox"            # answer text on stdout, progress on stderr
jarvis ask "summarise my inbox" > answer.txt
```

The CLI still contains no orchestration: it sends an objective and no identity, and the daemon resolves
the workspace and user from its own seeded rows. A client-supplied workspace identifier would be a claim
rather than proof of access.

**Runs now execute.** Set `daemon.executor_model` and the daemon drives a run to a terminal state:

```toml
[daemon]
http_enabled = true
http_port = 8765
executor_model = "scripted"
```

The two values this build implements are `scripted` and `openai-compatible`. `scripted` is a
**deterministic local model** that needs no network and no credential. It says in its own answer that no
language model is configured, so a misconfigured daemon cannot look like a working one. A name the build
does not implement stops the daemon at startup with an actionable error rather than being discovered when
the first run is started.

With the executor enabled, `jarvis ask` completes in about a second: answer text on stdout, progress on
stderr, and exit `0` for a completed run. The run's events are durable, so a client that reconnects
replays from its position rather than re-reading the whole answer.

**Answering with a real model.** `openai-compatible` reaches any server speaking
`POST /v1/chat/completions` — OpenAI itself, or a local Ollama server:

```toml
[daemon]
http_enabled = true
http_port = 8765
executor_model = "openai-compatible"                          # selects the transport
executor_base_url = "http://localhost:11434/v1"               # the server
executor_model_name = "qwen3:8b"                              # the model id sent to it
executor_api_key_ref = "C:/jarvis/model.key"                  # a FILE holding the key
```

`executor_model` selects the **transport** and `executor_model_name` is the **model id** the provider
receives; they are separate settings, so wanting a different model does not change how JARVIS talks to the
server. **The key is a path to a file, never the key itself**, because the configuration document is
printed and round-tripped by routine commands. Ollama ignores the key's value but requires one to be
present, so a local server can use a placeholder in that file.

A **partial** provider is refused rather than corrected: selecting `openai-compatible` while any
coordinate is missing stops the daemon at startup naming the missing setting, because a half-configured
provider would otherwise start cleanly and fail on the first run with an error from the *provider* instead
of from JARVIS. Provider coordinates without `openai-compatible` are refused too — a settings block that
has no effect is a worse outcome than an error, since it looks like it works. A relative key path is
refused; it must be absolute. `ADR-0121` records the reasoning.

The executor lives in `apps/jarvisd` rather than `jarvis-application`, because
`docs/architecture/repository-layout.md` allows the application layer to depend only on `jarvis-core`
and has no arrow from it into an adapter crate.

**Trying it with Ollama cloud models (no API key in this repository).** A local Ollama that is signed in
proxies its `:cloud` models, so the OpenAI-compatible endpoint above needs no real key:

```powershell
$env:JARVIS_HTTP_ENABLED = "true"
$env:JARVIS_EXECUTOR_MODEL = "openai-compatible"
$env:JARVIS_EXECUTOR_BASE_URL = "http://localhost:11434/v1"
$env:JARVIS_EXECUTOR_MODEL_NAME = "glm-5.3:cloud"
$env:JARVIS_EXECUTOR_API_KEY_REF = "C:/path/to/a/file/containing/the/word/ollama"
jarvisd --root C:/path/to/a/scratch/profile
```

### Tools and approvals

The model is offered the daemon's tools: file read/list inside `daemon.tool_workspace_roots`, memory proposals,
any configured MCP servers, and `jarvis.web.fetch` (a guarded read of a public web page, which runs without asking). A
tool the policy holds — code, anything with an external effect, or anything you set to `ask` — **parks the run** until you
decide it:

```powershell
jarvis ask "Run a snippet that prints 6*7."                            # exits 11: waiting for approval
jarvis approvals list                                                   # shows the tool and its exact arguments
jarvis approvals approve                                                # confirms, runs it once, prints the answer
jarvis approvals deny                                                   # the run answers that you declined
```

`jarvis cancel RUN` (a prefix is enough) or `jarvis cancel --all` is the kill switch: it stops a run that is waiting for you
at once and one that is working at its next step, and withdraws any approval the run was waiting on.

`approve` shows the arguments and asks; without a terminal it needs `--yes`. A held action survives a daemon
restart. See `ADR-0130`.

**Letting the model run code.** Pull an image once (the sandbox never pulls), then name it and its interpreter:

```toml
[daemon]
code_sandbox_image = "node:22-alpine"
code_sandbox_interpreter = ["node", "-e"]      # the snippet is appended as the last argument; there is no shell
```

(or `JARVIS_CODE_SANDBOX_IMAGE` and `JARVIS_CODE_SANDBOX_INTERPRETER="node -e"`). `jarvis.code.run` then appears in
`jarvis tools list`. Each snippet runs in a throwaway container with no network, a read-only filesystem, and 30
seconds, and **every run waits for your approval** — you read the code first — unless you trust it:

```toml
[policy]
trust = ["jarvis.code.run"]                    # or: jarvis init --code-image ... --trust-code
```

Trust is yours to give per tool, never given by default, and never applies to a tool that communicates externally; a
`deny` or an `ask` override for the same tool wins. To be asked before every web page instead, set
`[policy.approval] "jarvis.web.fetch" = "ask"`. See `ADR-0131`, `ADR-0133`.

### Watching it work

`jarvis hud` opens the JARVIS console in your browser: an animated orb (idle, working, listening, speaking, amber when
something **needs you**), a streaming conversation with the assistant, the same lists as `jarvis watch`, and **Stop**
buttons. Press the microphone (or `M`) to talk, turn on **Speak answers**, or enable the **wake word** and say
"Jarvis, ..."; "Jarvis, stop" cancels everything running and Escape (or clicking the orb) silences it. Voice uses your
browser's own speech support (Chrome or Edge); the browser's recognizer may send audio to its vendor's service. It can watch and stop
work but not approve it (the decision code is delivered to a private file, so the display shows you the command).
See `ADR-0135`.

`jarvis watch` is a live screen of the work: what is **waiting for you** (with the exact approve/deny commands), what is
**working** (runs and sub-agents, with ages and the cancel commands), what is **scheduled** next, and what just
**finished**. `jarvis watch --once` prints it once.

### Sub-agents

The assistant can hand a bounded task to a sub-agent (`jarvis.agent.delegate`) and collect the answer
(`jarvis.agent.result`), several at once with `background: true`. A sub-agent is an ordinary run: same tools, same
approvals (it parks if it needs one — decide it with `jarvis approvals`), shown in `jarvis runs` as `[sub-agent]`,
stopped with `jarvis cancel`. It cannot start further sub-agents, and at most four run at once. See `ADR-0134`.

### Tasks that run while you are away

```powershell
jarvis schedule add "Summarise what changed in my notes folder." --every 24h
jarvis schedule add "Remind me to renew the domain." --at 2026-12-01T09:00:00Z
jarvis schedule list
jarvis runs                     # what each run answered; --full for the whole answer
jarvis schedule pause 01a1      # an identifier prefix is enough when it is unambiguous
```

A scheduled task is an ordinary run started later, so it has **no extra authority**: if it wants a tool that needs your
approval it waits, and `jarvis approvals list` shows it. A task is skipped (and the skip counted) while its previous run is
still waiting on you, and a machine that was off owes one fire, not a backlog. There is no "daily at 08:00" yet: it needs
time-zone and daylight-saving handling (`P6-003`) and is not approximated. See `ADR-0132`.

### Chatting (multi-turn)

`jarvis chat` needs the same configuration as `ask`:

```powershell
jarvis chat
jarvis> what is on my calendar
jarvis> and the second one
jarvis> :quit
```

Each line starts a **new run in the same session**, so a conversation is a sequence of runs, not one
long-lived run. A run settles once and a settled run emits no further events, so a conversation cannot
be a single run; ADR-0014 records the decision.

The **daemon** replays the session's transcript into each model call, and the session identifier is
printed on the first turn. It is a value the daemon issued — the client never invents one, because a
session identifier is guessable and the daemon refuses a session belonging to another workspace as
`404` rather than confirming that somebody else's conversation exists.

Two limits are worth knowing:

- **History is bounded to the newest turns.** A transcript grows without limit and a model's context
  does not, so the window is chosen from the end and a turn the budget cannot hold is excluded with a
  recorded reason. Summarisation of long conversations is `P4-007`.
- **A conversation cannot yet be resumed from a new process.** The identifier is printed so it can be
  found later, but the inspect and export surface that would reopen one is `P4-008`. Continuing works
  within a single `chat` session.

### What a run stores

A run is durable in three places, and they answer different questions:

- `agent_runs` is the state machine: where the run is, and how it settled.
- `run_events` is the stream a client replays, ordered per run with `UNIQUE (run_id, sequence)`.
- `messages` is the conversation: the user's question and, when the run completes, the answer.

The user's message is written **in the same transaction** that creates the run, so an accepted message
cannot be lost — it exists before the run does. The assistant's answer is read back from the run's own
`output_completed` event rather than kept in memory, so the transcript and the event stream cannot
disagree about what was said. A failed run records the question and no answer, because there is no answer
to record.

### Portable Mode

Pass an absolute root to keep everything inside one directory:

```powershell
cargo run -p jarvisd -- --root C:\jarvis-portable
cargo run -p jarvis-cli -- status --root C:\jarvis-portable
```

Portable mode creates no service, PATH, or registry change; every managed file stays
inside the root. A relative root is refused rather than resolved against the current
directory. Profiles in different roots cannot collide: the local endpoint is derived
from the root as well as the profile name, so a portable `default` and a native
`default` can run at the same time without contending for one socket or pipe.

`jarvis service` reports what a per-user service reconcile would do. It is read-only:
it prints the definition path and launch command, classifies the existing definition
as `absent`, `current`, `drifted`, or `foreign`, and refuses in portable mode. A
`foreign` definition is never replaced — that requires explicit operator action.

`jarvis doctor` never changes anything unless `--repair` is passed, and any repair is re-verified before its result is reported. Its exit code tells automation what happened: `0` passed, `6` warnings, `7` failed, `8` a repair did not verify. A profile that has never been started reports **warnings**, not success, because there is no daemon, credential, or log record yet.

The daemon prints its readiness line at startup and transitions through `booting` to `ready`; stop it with Ctrl+C, which performs a graceful shutdown rather than an abrupt exit.

Logs are one JSON object per line at `<logs>/jarvisd.jsonl`. Every line passes through the redactor before it is written, so known secrets, credential-shaped key/value text, `Bearer` values, and URL userinfo passwords are masked even if a call site forgets a rule. Lines longer than 8 KiB are truncated on a UTF-8 boundary and report the omitted byte count.

The CLI exits `0` on success, `2` for a usage error, `3` when the daemon is unreachable or not ready, `4` when authentication or authorization fails, `5` for a rejected request, `9` when a run was cancelled, and `10` when an accepted run failed. Cancellation and failure are distinct from `0` and from `5` on purpose: a script that treats "the run ran" as success would otherwise record a cancelled run as a completed task.

The daemon issues a 32-byte credential for its profile on first run and stores it beside the profile configuration. Clients only ever read that value; if it is missing, `jarvis` reports that the daemon has not issued one and exits `3` rather than creating a value. Otherwise any local process could choose the secret the daemon trusts. Do not copy the credential between machines or profiles.

## Orientation

Read in order:

1. [AGENTS.md](../../AGENTS.md)
2. [Product requirements](../product/requirements.md)
3. [Architecture overview](../architecture/overview.md)
4. [Repository layout](../architecture/repository-layout.md)
5. [Roadmap](../../ROADMAP.md)
6. [TODO.md](../../TODO.md)
7. [Testing](testing.md)
8. [Definition of done](definition-of-done.md)

For an external integration, also read [external-research.md](external-research.md) and invoke the repository's `external-integration-research` skill when supported.

## Phase 1 Prerequisites

The first implementation slice should require only:

- Git
- a Rust toolchain installed through rustup
- platform C/C++ build prerequisites required by selected Rust crates
- PowerShell on Windows or a POSIX shell on macOS/Linux

The committed [rust-toolchain.toml](../../rust-toolchain.toml) is the exact Rust version authority. Rustup installs or selects that toolchain automatically when commands run from this repository.

Node.js is introduced only when the web/Tauri UI begins. Python is introduced only for isolated Python runtime workers or the reference prototype. Neither is a core runtime prerequisite.

## Foundation: `P1-001`

The first slice created the smallest compilable workspace:

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
rustfmt.toml                    only if non-default policy is justified
deny.toml                       when dependency policy is enabled
apps/jarvisd/
apps/jarvis-cli/
crates/jarvis-core/
crates/jarvis-application/
crates/jarvis-protocol/
crates/jarvis-storage/
```

The initial binaries printed package versions and exited. They did not contain fake model or tool implementations. Package manifests encode the dependency direction documented in [repository-layout.md](../architecture/repository-layout.md).

Before adding dependencies:

1. Name the use and owning crate.
2. Prefer established workspace dependencies with narrow features.
3. Check license and advisories.
4. Keep domain crates free of framework/provider dependencies.

The root [Cargo.toml](../../Cargo.toml) owns the workspace lint policy. The pinned default rustfmt style applies; Rust unsafe code is forbidden, unused results are denied, public documentation is warned, and Clippy's `all` and `pedantic` groups run with selected placeholder and panic shortcuts denied.

Required baseline after `P1-001`:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Add CI in `P1-002`; do not overload the first slice with service installation, SQL, HTTP, or models.

## Implementation Loop

For each TODO item:

1. Identify the owning module and nearest acceptance test.
2. Read only enough surrounding code/docs to state a falsifiable hypothesis.
3. Make the smallest coherent behavior change.
4. Run the narrow test immediately.
5. Repair that slice before expanding scope.
6. Run package/workspace quality gates.
7. Update docs and the TODO checkbox only after evidence passes.

Keep a runnable daemon/client path from Phase 1 onward. Libraries that no binary exercises are not integrated.

## Configuration And Local Data

Never write local state into the repository. Use OS application directories through the Phase 1 path service. Tests use isolated temporary profiles.

Future environment variables follow a documented prefix such as `JARVIS_`; sensitive variables contain secret values only when the deployment explicitly selects environment-backed secrets. Examples use placeholders and `.env.example`, never `.env`.

The Phase 1 configuration is `<config>/config.toml`. Precedence is built-in defaults, then the file, then the explicit environment allowlist. Schema v1 is:

```toml
schema_version = 1

[profile]
name = "default"

[logging]
level = "info"

[daemon]
shutdown_timeout_seconds = 15
```

Allowed overrides are `JARVIS_PROFILE_NAME`, `JARVIS_LOG_LEVEL`, and `JARVIS_SHUTDOWN_TIMEOUT_SECONDS`. Unknown TOML keys and unknown `JARVIS_*` variables are errors. Missing files use defaults without writing; schema v0 is migrated in memory and must be saved explicitly. Newer schemas fail closed. Configuration writes validate first, replace atomically, and retain no secret values.

### Configuring tool approval

The `[policy]` section decides which tools need a human, and how much risk the workspace permits at all:

```toml
[policy]
max_risk = "high"              # the highest risk permitted at all
approval_threshold = "moderate" # risk at which approval is always required
deny = ["jarvis.mail.send"]     # refused outright, whatever the grants say

[policy.approval]               # per-tool, by identifier
"jarvis.files.read" = "ask"     # a read the workspace would allow now waits for a human
"mcp.github.create_issue" = "auto"  # ignored: see the direction rule below
```

The four approval values are `auto` (run when scoped), `policy` (the workspace's threshold decides),
`ask` (always ask, whatever the threshold says), and `deny` (never run, even with an approval a human
could give). Risk levels are `minimal`, `low`, `moderate`, and `high`.

**An override can only tighten, never relax.** `[policy.approval]` is applied as a maximum against the
tool's own declaration, so `"jarvis.mail.send" = "auto"` cannot remove an `ask` the tool's author
declared — the entry is inert rather than a removed guard. That is deliberate (`ADR-0017`, `ADR-0122`): a
configuration file that could relax a tool's own approval policy would be a way to disable a security
control from a text file. To loosen a tool, change the tool's declaration, not the workspace.

**Two mistakes are refused at startup rather than starting a daemon that misbehaves:**

- `approval_threshold` above `max_risk` — every risk that could be approved is already refused, so the
  threshold can never take effect;
- a blank identifier in `deny` or `[policy.approval]` — it applies to nothing while reading as a
  configured restriction.

A policy entry naming a tool that is **not currently registered** is not refused: MCP servers are
discovered at startup and one may be down, so an inert entry is the honest outcome rather than a daemon
that will not start.

Omitting `[policy]` entirely gives the workspace defaults, which permit the full risk range and ask for
approval from `moderate` up.

### Seeing the policy in force

A configuration file is the **input**; what the daemon enforces is derived from it (an override is applied as
a maximum, a denial short-circuits several checks, and the actor's scopes are derived rather than
configured). So `jarvis tools` reads the posture back from the running daemon:

```powershell
jarvis tools list
jarvis tools list --json
jarvis tools preview jarvis.files.read
jarvis tools preview jarvis.mail.send --escalation bulk --channel voice
```

`list` shows each tool's effective approval policy, marking an override as `auto -> ask (overridden)` rather
than printing the configured value as if it were in force. `preview` reports the decision a call **would**
produce — `allow`, `require_approval`, or `deny` — with the stable reason code, the effective risk beside the
declared one, and the context signals that raised it.

It is a decision and not a prediction: the daemon computes it with the same pure `evaluate` the tool-call
path uses, so it is exact for the context supplied. It records no call and consumes no idempotency key, so
inspecting a policy cannot fill the ledger or make a later real call a duplicate.

```powershell
jarvis tools preview jarvis.mail.send --escalation bulk
# jarvis.mail.send: require_approval
#   reason            approval_required
#   risk              high (declared moderate)
#   escalated by      bulk
#   approval needs    present
```

The flags a preview accepts are only the parts of the call the caller knows: `--channel`, `--strength`, and
one `--escalation` per signal. Scopes and the workspace policy come from the daemon, so a preview cannot be
used to ask what a different set of permissions would decide.

**Two empty-looking answers are different problems.** `jarvis tools list` exits `4` and prints the remedy
when the daemon has **no tool surface at all** (no roots granted and no MCP servers configured), rather than
printing an empty list — "nothing is configured" and "this daemon cannot serve tools" need different fixes.

## Prototype

The [example](../../example/readme.md) can be run separately to study behavior, subject to its dependencies, terms, and local credential handling. Do not run it automatically during production setup or tests. Do not read or commit its `config/api_keys.json` or `memory/long_term.json`.

Use [the migration inventory](../migration/python-prototype.md) to translate behaviors into clean-room tests and Rust ownership.

## Before You Stop

- report commands actually run and their result
- distinguish implemented, partial, researched, and unverified work
- leave no required server/watch process running unintentionally
- leave the first incomplete TODO task explicit
- do not commit unless the user asks