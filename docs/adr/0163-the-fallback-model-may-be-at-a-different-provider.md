# ADR-0163: The fallback model may be at a different provider

Status: Accepted
Date: 2026-10-10

## Context

`ADR-0159` asks a second model at the same provider when the first is rate limited or overloaded. That does nothing when the limit or the outage belongs to the provider (an account-wide limit, spent credit, a provider incident), which is exactly when the owner's long autonomous runs stop.

## Decision

1. **Two optional settings**, `daemon.executor_fallback_base_url` and `daemon.executor_fallback_api_key_ref`, beside `executor_fallback_model_name`. With the address and a readable key, the fallback model is asked at that provider (the same OpenAI-compatible adapter, built the same way, so the same address checks apply). Without them nothing changes.
2. **What falls back.** At a different provider: a rate limit, an overload, a transient failure that outlasted the adapter's own retries, and exhausted credit. Never a bad credential, a missing model, an oversized context, a content refusal or an invalid request: those are the owner's to fix, and sending the run's text to another provider because of them would be a surprise. As before, only a failure that arrives **before any stream opens** falls back, so nothing is said twice; one try at each provider; the next call tries the main provider again.
3. **Consent is in the setting.** The run's text goes to a provider only because the owner set its address and key; the settings help and `docs/user/settings.md` say so. The address must be http(s) with no login in it, the key file absolute, and the address needs a fallback model name (`validate_fallback_provider`); a key without an address is refused, while an address without its key is allowed so the settings screen can save one field at a time.
4. **Optional means optional.** An unreadable or empty fallback key file costs the fallback, not the daemon: it is logged once at startup and the main provider runs alone. A rejected fallback address fails the build with the field named, like the main one.
5. **Keys.** `jarvis keys set|remove|status|test fallback` and a "Fallback key" row in Settings, stored like the others (private file, never shown again).

## Consequences

A run survives a provider-level limit or outage when a second provider is configured. Not built: choosing among several fallbacks, a provider with a different wire protocol (only OpenAI-compatible endpoints), carrying the main model's tool-call quirks over (a different model may call tools differently), and **a live run against a real second provider** — the behaviour is tested with scripted models and the address checks with unit tests only.