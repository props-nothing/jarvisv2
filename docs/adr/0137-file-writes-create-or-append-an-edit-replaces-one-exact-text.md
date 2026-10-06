# ADR-0137: File writes create or append; an edit replaces one exact text

Status: Accepted
Date: 2026-10-06

## Context

JARVIS could read and list files in the folders the owner granted, but not change them, so "write me a note", "fix that
typo" or "start this project" ended with text for the owner to paste. An assistant that does the work needs to write.
The owner's own files are the one place a mistaken write is hard to undo, so the design must make the common case
frictionless and the destructive case impossible or visible.

## Decision

1. **`jarvis.files.write` creates a new file or appends, and can never replace content.** `mode: create` (default)
   refuses when the file exists; `mode: append` adds to the end, creating the file if missing. Missing folders are
   created. It declares a write at risk 1, so under the default policy it **runs without asking**: the worst it can do is
   add text inside a granted folder.
2. **`jarvis.files.edit` replaces one exact text in an existing file.** The text must occur exactly once (zero or several
   matches are refused with a reason the model can act on); a CRLF file matches LF-written text. The new file is written
   beside the original and renamed over it, so a crash leaves the old or the new file, never half. It declares a write at
   risk 2, so it is **asked about once** under the default policy (the owner answers with one click, command or word,
   `ADR-0136`); `[policy] trust` waives that.
3. **Confinement is unchanged**: both go through the same directory handles as reads (`ADR-0020`), so `..`, absolute
   paths and links out of a granted folder are refused. Content is capped at 4,000 characters per call (a longer file is
   written in appends) so a held call's arguments always fit the stored approval.
4. **Not served to remote MCP callers.** An external client reaching these tools would change the owner's files on an
   allowlist entry alone, so `servable_definitions` removes them from the served surface.
5. **A failed call tells the model why.** The tool's reason ("not found", "occurs 2 times", "file exists") is fenced and
   returned to the model, including after an approval, so it corrects itself instead of guessing.
6. **A repeated identical request is refused readably.** One approval exists per action per run; asking the exact same
   thing again returns a sentence naming the earlier answer instead of a database error.

## Consequences

- Delete, move and overwrite remain out of scope; each needs its own undo design.
- An edit is one approval per change. A batch of edits is several approvals; revisit with a per-run "allow edits in this
  folder" choice if that proves noisy.
- Tests: create, parents, no overwrite, append, traversal/absolute/link refusal for write and edit, oversize, unique
  replace, not-found/ambiguous/identical, CRLF, no temporary file left, MCP surface exclusion, repeated request.