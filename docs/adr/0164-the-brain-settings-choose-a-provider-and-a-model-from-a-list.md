# ADR-0164: The Brain settings choose a provider and a model from a list

Status: Accepted
Date: 2026-10-10

## Context

Choosing a model meant typing an address and a model name from memory into text boxes, with the keys listed above them, and finding out whether it worked only on the first run. A second provider (`ADR-0163`) made that worse: four more fields.

## Decision

1. **One card per model, main and fallback:** provider, address, key, model, with Load models, Test and Save. Providers are presets (Ollama on this computer, Ollama Cloud, OpenAI, Google Gemini, Anthropic, other OpenAI-compatible) that only fill in the address and say where the key comes from. The fallback card adds "same provider as the main model".
2. **Models are asked of the provider, never kept.** `POST /api/v1/settings/models` makes `GET {address}/models` from the daemon (so a stored key never reaches the page) and returns the identifiers, likely chat models first; the model box is type-to-filter and still accepts any typed name, so a provider that does not list is not blocked. With `test_model` it also makes one chat call capped at 64 tokens and says what failed (address, key, model, limit). It says tool use was not checked.
3. **A stored key goes only to the address it was stored for** (`model_probe::resolve`). Another address needs a key typed in the same request. The page applies the same rule to Save: changing the address without pasting that provider's key is refused, so the old provider's key is never left standing for a new one. A local server gets a placeholder key.
4. **The web-search key moves to its own "Web search" tab**, out of Brain; the Google tab's intro no longer says it is read-only.
5. **Only OpenAI-compatible endpoints.** Anthropic is offered through its documented compatibility layer with the vendor's caveat (a way to try Claude, no prompt caching, thinking not returned). A native Anthropic adapter is not built.

## Consequences

The owner can switch provider and model from a list, check it works, and set a fallback at another provider without editing text. Saving still needs a restart to apply. Not built: a tool-calling probe, hot-swapping the model without a restart, price or context-size information, native Anthropic and Gemini adapters, and live checks against OpenAI, Gemini, Anthropic and Ollama Cloud (no keys were available; only local Ollama was exercised).