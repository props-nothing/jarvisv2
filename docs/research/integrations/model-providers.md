---
integration: model-providers
status: implemented-minimal
last_verified: 2026-10-10
owners: []
selected_spec_version: "each vendor's documented OpenAI-compatible surface; no version pinned (see Versions)"
selected_sdk: null
---

# Model providers behind the OpenAI-compatible adapter (choosing one, listing its models)

## Scope

What the Settings screen needs to let the owner pick a provider and a model instead of typing addresses and names (`ADR-0164`): the OpenAI-compatible base address of each offered provider, how its key is sent, and how to list its models (`GET {base}/models`). Out of scope: a native Anthropic or Gemini adapter, embeddings, pricing, and tool-call quirks per provider. The chat call itself is recorded in [openai-compatible-model-api.md](openai-compatible-model-api.md).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| OpenAI list models | https://developers.openai.com/api/reference/resources/models/methods/list (redirect of platform.openai.com/docs/api-reference/models/list; the page points to https://developers.openai.com/llms.txt) | 2026-10-10 | `GET /v1/models` shape |
| Ollama OpenAI compatibility | https://docs.ollama.com/api/openai-compatibility.md (index: https://docs.ollama.com/llms.txt) | 2026-10-10 | local and cloud addresses, `/v1/models` |
| Ollama authentication | https://docs.ollama.com/api/authentication.md | 2026-10-10 | cloud key, Bearer header |
| Gemini OpenAI compatibility | https://ai.google.dev/gemini-api/docs/openai (fetched as HTML; the page's own `curl` example lists `.../v1beta/openai/models`) | 2026-10-10 | address, model listing |
| Anthropic OpenAI SDK compatibility | https://platform.claude.com/docs/en/cli-sdks-libraries/libraries/openai-sdk | 2026-10-10 | address, limitations |
| Anthropic list models (native) | https://platform.claude.com/docs/en/api/models/list | 2026-10-10 | native `GET /v1/models` |
| Anthropic versioning | https://platform.claude.com/docs/en/api/versioning | 2026-10-10 | `anthropic-version: 2023-06-01` |

## Verified Contract

### Operations And Transport

- **OpenAI:** `GET https://api.openai.com/v1/models`, `Authorization: Bearer`. Body `{"object":"list","data":[{"id","created","object":"model","owned_by","shutdown_date"}]}`. The list is every model the account can reach, **including non-chat models** (embeddings, speech and so on); it says nothing about tool support.
- **Ollama:** `/v1/models` is a supported endpoint; `created` is the model's last modification and `owned_by` the Ollama username (default `library`). Local server `http://localhost:11434/v1`: "the client requires an API key value, but Ollama ignores it". Cloud `https://ollama.com/v1` "requires an API key" sent as `Authorization: Bearer`. Cloud models can also be used through the local server once signed in.
- **Gemini:** base `https://generativelanguage.googleapis.com/v1beta/openai/`; the key is the Gemini API key (`aistudio.google.com/apikey`); the page's own example lists models with `curl https://generativelanguage.googleapis.com/v1beta/openai/models`. Chat examples use bare model names (`gemini-3.8-flash`).
- **Anthropic:** base `https://api.anthropic.com/v1/`, key from `platform.claude.com/settings/keys`, sent by the OpenAI SDK (so as Bearer) for chat. The vendor says the layer "is primarily intended to test and compare model capabilities, and is not considered a long-term or production-ready solution"; limitations: `strict` tool schemas ignored, **no prompt caching**, system messages hoisted into one `system` field, **thinking is not returned**. The native `GET /v1/models` is documented with `anthropic-version` (and optional `anthropic-workspace-id` for keys that can reach several workspaces), cursor paging (`limit` 1 to 1000, default 20).

### Authentication And Authorization

Bearer key per provider. JARVIS sends only `Authorization: Bearer` (the adapter's transport carries no other header by design). Whether Anthropic's `/v1/models` accepts a Bearer key through the compatibility layer is **not documented** on the pages read.

### Limits And Failure Semantics

No limits were documented for listing. JARVIS bounds it itself: one attempt, 20 s, at most 500 identifiers, identifiers that are not valid model names dropped. A test call is one chat completion capped at 64 output tokens, 45 s, one attempt.

### Data And Compliance

Listing sends only the key and no content. A test sends one fixed sentence. A fallback provider receives the run's text whenever the main provider is limited (`ADR-0163`).

### Versions And Deprecations

OpenAI marks models with a `shutdown_date`; Anthropic lists `lifecycle` (active, deprecated, retired). Neither is used yet. Model names are never stored in JARVIS's presets because they go stale; they are asked of the provider.

## JARVIS Mapping

- `jarvis-models`: `OpenAiCompatibleProvider::list_models` (one attempt; errors classified like a call; the provider's own text never carried; a leading `models/` removed from identifiers, which is how Google names them in its native API; this was not verified against Gemini's compatible endpoint).
- Daemon: `POST /api/v1/settings/models` (`model_probe.rs`): the daemon makes the call; a **stored key is only ever sent to the address it was stored for**; a typed key goes with the address typed with it; a server on this machine needs no key.
- Presets (`model_probe::PRESETS`): addresses and key help only.

## Decisions

- Presets are addresses, not models. Models come from the provider so they cannot go stale.
- Anthropic and Gemini are offered through their compatible endpoints, with the vendor's own caveat shown for Anthropic. A native adapter is a separate, larger decision.
- The test checks that a model **answers**, not that it calls tools well; that is said in the result.

## Rejected Alternatives

- A hard-coded model catalogue: stale within weeks.
- The browser calling providers itself: the stored key would have to reach the page, and the page's policy forbids outside origins.
- Sending `x-api-key` and `anthropic-version` to Anthropic from the shared transport: the transport admits only the headers the adapter uses, so a secret cannot be put into an arbitrary header.

## Verification Plan

- Done: scripted-transport contract tests (list shape, de-duplication, error classes, key never in an error); resolve-rule falsification tests; preset checks; live against the owner's local Ollama (5 models listed, a test call passed in 26.7 s) and the live refusal of another address without a typed key.
- **Not done:** a live listing or test against OpenAI, Gemini, Anthropic or Ollama Cloud (no keys); Gemini's list identifier format; whether Anthropic's compatibility layer serves `/v1/models` with a Bearer key. If it does not, the screen says so and the model name can still be typed.

## Unresolved

- Anthropic `/models` with Bearer (blocks only the dropdown for Anthropic, not use).
- Gemini identifier prefix (blocks nothing: bare names are documented to work for chat).