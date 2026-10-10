# Evaluating the assistant

`jarvis eval` answers "did that change make JARVIS better or worse at my work?". It runs a file of prompts against your running JARVIS, checks each answer against what you said should be true, and compares with the last time you ran the same file.

```text
jarvis eval run evals/basics.toml          # nine plain cases: answers, tool choice, honesty, what a trivial request costs
jarvis eval run evals/gates.toml           # sending mail must stop and ask
jarvis eval run my-prospecting.toml --case dutch-offer
jarvis eval history                        # every saved run
```

Each case is a normal message in a new conversation, so it uses real tokens and goes through your real permissions. The harness stops (cancels) a case that waits for your approval or runs out of time, so nothing is left on your screen. The suites that ship are read-only: nothing is sent, drafted, written or scheduled.

## Writing a suite

```toml
name = "prospecting"                       # letters, digits, - and _; names the saved history
description = "What I actually ask it to do."

[[case]]
id = "dutch-intro"
prompt = "Schrijf een korte, eerlijke introductie in het Nederlands voor een tandartspraktijk."
[case.expect]
contains_any = ["tandarts", "praktijk"]    # at least one
not_contains = ["The ", " is the "]        # none of these (English leaking in)
max_tool_calls = 0
max_answer_chars = 900
max_seconds = 60
```

| Expectation | Meaning |
| --- | --- |
| `contains = [...]` | every one is in the answer (any capitalisation) |
| `contains_any = [...]` | at least one is |
| `not_contains = [...]` | none is |
| `tools_used = [...]` | each was requested; `web.fetch` and `jarvis.web.fetch` mean the same |
| `tools_not_used = [...]` | none was requested |
| `max_tool_calls`, `max_input_tokens`, `max_answer_chars`, `max_seconds` | limits |
| `parks_for_approval = true` | the run must stop for your approval (it is then cancelled) |

A case with no expectations only has to complete. Unknown keys are refused, so a typo cannot silently check nothing.

## Reading the result

```text
suite basics (model glm-5.3-flash)
  pass  arithmetic                       1 s   0 tools    6,650 in /    21 out
  FAIL  fetches-a-page                   2 s   1 tools   13,451 in /    66 out
          - contains "Example Domain": not in the answer
8/9 passed, 94,944 tokens in, 1,127 out, 17 s
against the previous run (2026-10-10T15:58, 8/9 passed): 0 regression(s), 1 fixed, input tokens +7%
```

`jarvis eval run` exits with a non-zero status when any case fails. Results are saved under your data folder (`evals/<name>/`, one JSON file per full run, answers cut to 500 characters). A run with `--case` is shown but not saved.

## What it cannot tell you

- The checks are plain text. They cannot read negation: "has been deleted" is also in "nothing has been deleted", so name a first-person claim ("I deleted") instead.
- A tool being *requested* is not it *succeeding*.
- A model gives different answers each time. One failure is a signal; run it a few times before concluding.
- Whether an answer is persuasive or well written is not measured. Use the suite for what can be checked (language, tools used, honesty, cost) and read the rest yourself.
- What stops for approval depends on your own permission settings (Settings, Permissions). A tool you have set to run without asking will not stop, and a case that expects it to will fail, which is the harness telling you something true.