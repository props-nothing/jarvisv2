# ADR-0165: A project can cap the tokens its scheduled work uses in a day

Status: Accepted
Date: 2026-10-10

## Context

Switching to a paid provider is now one click (`ADR-0164`), and scheduled tasks and sub-agents run without the owner there. `ADR-0155` capped how many *runs* a project's schedule may start, but a run's cost is its tokens, and one run can use a hundred times another. Every model call already records its own `usage_updated` event (`input_tokens`, `output_tokens`, `cached_input_tokens`); nothing added them up per project.

## Decision

1. **`daily_token_limit` per project** (migration 0020, 0 to 2,000,000,000, 0 meaning none), set like the run cap: `jarvis project add|set --token-limit N`, **Daily tokens** in the project window, `daily_token_limit` on the API. A model cannot change it (the project tool constructs both caps as unchanged).
2. **What is counted:** input plus output tokens from the `usage_updated` events of runs in conversations that belong to the project, in a rolling 24 hours (no time zone), `project_tokens_since`. Cached input is not added, because providers report it inside the input figure or beside it and summing could double count. The project reply carries `tokens_today` so the screen and CLI show use whether or not a cap is set.
3. **Where it is enforced:** the scheduler, beside the run cap (`over_daily_cap`). A scheduled fire for a project at or over its tokens is skipped and counted, like a paused project. The owner's own messages are never refused, and a run in progress is never stopped.
4. **It fails closed.** The run cap fails open if its count cannot be read; the token cap guards spend, so when the tokens used cannot be read the fire is held.

5. **Usage had never been recorded, and is now.** Building this showed that no run had a `usage_updated` event: OpenAI-style providers (and Ollama) send token usage in a chunk *after* the finish reason, and the adapter stopped reading at the finish reason. The finish is now held until the usage chunk, `[DONE]`, the connection closing, or two seconds (`openai-compatible-model-api.md`, finding of 2026-10-10). Runs before this change have no usage and count as zero.

## Consequences

An unattended project cannot keep spending past its daily cap, but a single run can overshoot it (the check is when a run is *started*), and sub-agents started from a run are not separately stopped. Not built: stopping a run mid-way at the cap, prices or money (provider prices are not in `/models`, and a built-in table would go stale), a workspace-wide cap, and caps for work that is not in a project.