# ADR-0159: Files can be moved, and a limited model can be swapped for another

Status: Accepted
Date: 2026-10-10

## Context

Two gaps showed up in running JARVIS as a project manager. It could create and edit files but not organise them: filing a quote under `sent/`, or archiving a finished folder, meant copying and leaving the original. And a provider rate limit (`ADR-0156` makes a run wait it out) usually belongs to
one model, not the whole account, so waiting is often worse than asking a sibling model.

## Decision

1. **`jarvis.files.move`** moves or renames one file or folder inside a granted folder, to a new path in the same root; missing folders are created. It **never replaces**: a target that exists is refused. A file is moved by hard-linking it to the new name (which fails if the name is taken, even if something creates it a moment after the check) and removing
   the old name; where the filesystem cannot link, by a rename after an existence check. It never crosses roots (that would be a copy and a delete), never moves a folder into itself, and resolves both paths through the root's confinement handle like every other file tool, so neither can leave it. Effect `write`, risk low, so it runs without asking like creating a file; it is not
   offered over the inbound MCP endpoint, like the other tools that change the owner's files.
2. **A fallback model.** `daemon.executor_fallback_model_name` names a second model at the same provider. When a call is refused with a rate limit or an overload **before any stream opened** (after the adapter's own quick retries), the same request is made once for the fallback model, for that call only; the next call tries the main model again.
   It is a wrapper around the live gateway (`FallbackGateway`), so no run signature changed. Only those two refusals fall back: a bad key, a missing model, a context that is too large, a content refusal or an exhausted quota are reported as they are, and a request already for the fallback is not sent on again. If the fallback is limited too, the run's own wait-and-retry
   (`ADR-0156`) takes over.

## Consequences

A project run can keep its folders tidy without losing anything, and a limit on one model costs a call a second model rather than five minutes. The fallback may answer differently or less well than the main model for that call, and it is a different model reading the same context, which is the owner's choice by setting it. Not built: switching to a model at a different provider (each
model name is asked of the same endpoint with the same key), and a fallback for the voice or embedding models.