# ADR-0142: A run is bounded by progress and by you, not by small counts

Status: Accepted
Date: 2026-10-06

## Context

Asked to "create a new Next.js app", JARVIS failed at its sixteenth tool call with "the run failed" and no reason, with the work
nearly done. The budget (8 model calls, 16 tool calls) had been chosen for "one to decide, one to answer", and real work (building a
project, fixing a bug across files) is dozens of steps. Looking at that one run found a family of small limits that each cut real
work short, and a worse problem behind them: for minutes at a time nothing visible happened, because a reasoning model streams its
thinking separately from its answer and JARVIS ignored it, so a long think was indistinguishable from a hang. (One run reasoned for
166,000 characters, about 40,000 tokens, deciding how to style a page.)

## Decision

1. **Ceilings, not budgets.** A run may make up to 400 model calls and 1,200 tool calls: a safety net against a runaway that
   nobody is watching, not a limit on honest work. What stops a loop is the **repeat guard** (the same call four times in a row is
   not run and the model is told; the seventh stops the run as `repeating_the_same_call`) and **you** (Stop works at any time).
2. **Room to work.** One file write may be 60,000 characters (it was 4,000, which split files into appends that merged lines).
   That needed the **canonical intent** cap raised from 8,192 to 1,048,576 characters: it is only ever hashed (the database stores
   the digest), but it refused every call over about 8 KB with "a canonical intent exceeds 8192 characters", found by a test when
   the write limit was first raised. An **edit** stays at 2,000 and 4,000 characters: it asks first, and a pending approval holds
   its arguments in at most 8,192 bytes, so a larger one could never be shown to the person deciding; a big change is a write of
   the whole file. The context window a run assembles 64,000 tokens (it was 8,192); conversation history 40 turns (it was
   12); the model request timeout 15 minutes with a 5 minute stall bound (they were 2 minutes and 90 seconds, shorter than a
   model needs to write a large file); an API request body 256 KB (it was 16 KB).
3. **Reasoning is visible and bounded.** The model adapter counts reasoning and tool-call text without keeping it
   (`StreamEvent::Progress`), the executor records it as an `activity_updated` event at most every three seconds, and the console
   shows "thinking (~11k tokens)". A model that reasons past 40,000 characters without saying anything is cut off and asked once
   more to act, with `reasoning_effort: low`, and stays on low for the rest of the run, because a model that rambled once will again.
   A model that has begun answering is never cut off.
4. **You are kept informed.** The system prompt asks the model to say in one sentence what it is about to do and to give a one-line
   status as each stage finishes (these are spoken as they stream when the voice is on). The chat shows a live line: the step, what
   it is doing ("writing app/page.tsx", the file path comes from the tool call and nothing else of the arguments does), and the time.
   After a minute of quiet the voice says it is still working. A run that fails says why, in words, and offers **Continue**.

## Limits that remain

A message is at most 4,096 characters (a database check; lifting it needs a schema migration). An edit still asks for approval
(ADR-0137); a coding task with many edits is many clicks until a per-run "allow edits" exists. `reasoning_effort` is sent only after
an overrun; a setting to choose it is not built.

## Falsification

Tests: thirty tool calls complete (the old budget would have failed at sixteen); one call repeated is nudged then stopped with
`repeating_the_same_call`; silent reasoning and tool-call writing yield count-only progress and the reasoning text is nowhere in the
answer; a model that only reasons past the budget is cut off, and one already answering is not. Live: a multi-page Next.js request ran
past 50 tool calls, the console showed the thinking count and each file as it was written, and the reasoning limit fired and recovered.