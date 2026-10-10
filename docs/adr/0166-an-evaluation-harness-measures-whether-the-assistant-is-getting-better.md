# ADR-0166: An evaluation harness measures whether the assistant is getting better

Status: Accepted
Date: 2026-10-10

## Context

The acceptance gates assert that behaviour is correct. They do not say whether an answer is *good*, and every recent change (a prompt, a tool description, a model, a provider) was judged by feel. Token use was not even recorded until `ADR-0165`. With a second provider one command away, "is this model better for my work than that one" had no way to be answered.

## Decision

1. **`jarvis eval run SUITE.toml`** runs each case of a suite as an ordinary run in a fresh conversation, through the daemon's API, so it meets the real policy, approvals, tools and model. It reads the run's events back for what it answered, which tools it requested and how many tokens its model calls used, and scores it.
2. **Expectations need no second model.** Words the answer must (`contains`, `contains_any`) or must not (`not_contains`) contain, tools it must or must not request (`tools_used`, `tools_not_used`; `web.fetch` and `jarvis.web.fetch` are the same), limits on tool calls, input tokens, answer length and time, and `parks_for_approval` for a case that must stop and wait for the owner. The scorer is a pure function. A suite with an unknown key is refused, because a misspelt expectation would silently check nothing.
3. **A case that needs an approval is parked, counted and cancelled**, as is one that runs out of time, so a suite never leaves work on the owner''s screen. Suites should hold read-only work: the committed ones send, draft, write and create nothing.
4. **History makes it a measurement.** Each full run is saved under the data directory (`evals/<suite>/<time>.json`, answers cut to 500 characters) and compared with the previous full run of the same suite: regressions, fixed cases, and the change in input tokens. `jarvis eval history` lists them. A partial run (`--case`) is shown but not saved. The exit status is non-zero when any case fails.
5. **What it cannot do, said plainly.** The checks are plain text and cannot read negation (the first version of one case failed the correct answer "nothing has been deleted" for containing "has been deleted"), and a tool *requested* is not a tool *succeeded*. A model varies between runs, so one failure is a signal to look at, not proof. Quality that needs judgement (is this email persuasive) is not measured; that would need a second model or a person.

## Consequences

Two suites ship (`evals/basics.toml`, `evals/gates.toml`); the owner writes their own for their own work. Run live on 2026-10-10 against `glm-5.3-flash` on Ollama Cloud: `basics` 9 of 9 in 17 seconds, the harness failed a deliberately wrong case, stopped a timed-out run, and proved `gmail.send` parks for approval. Not built: repeating a case several times and reporting a pass rate, scoring by a second model, and a console view of the history.