# ADR-0148: The model can search the text of the files in a granted folder

Status: Accepted
Date: 2026-10-09

## Context

Working on a project, the model could list a folder and read one file at a time. To find where something is defined or used it had to open
files one by one, which spends calls and context and is how a long coding task gets slow. A search is the first thing a person reaches for.

## Decision

`jarvis.files.search` joins the filesystem adapter: read-only, risk 0, runs without asking, scope `files.read`, like read and list. Arguments:
`pattern` (literal, case-insensitive, within one line), optional `path` (default the granted root), optional `glob` of `*` and `?` on file names,
`max_results` (default 40, at most 100). It returns `{ file, line, text }` entries, how many files it read, and whether it stopped at a bound.

- **Confinement is the existing one.** Every directory listing, size check and read goes through the roots' handles (`WorkspaceRoots`,
  with one new method, `entry_kind`), so a link out of the granted folder is an error and is never followed. Falsification test: a junction (a
  symlink on Unix) inside the root pointing at a folder with the searched word outside it; the outside file must not appear.
- **Bounded, and says so.** At most 5,000 files, 32 MiB read, 256 KiB per file, ten seconds and 28 KiB of output; hitting any of them (or the match
  limit) sets `truncated`, because a silently partial answer reads as "nothing else matches".
- **Skips what is never meant:** `.git`, `.hg`, `.svn`, `node_modules`, `target`, `.next`, `dist`, `build`, `__pycache__`, `.venv`; binary files (a NUL
  in the first KiB); files that are not UTF-8.
- Literal matching, not regular expressions: it needs no new dependency and cannot be made to run away on a pattern. A regex mode is a later,
  separately bounded step if literal search proves too weak.

## Consequences

Verified live: asked to find where `PageHero` appears in a small project (with a decoy inside `node_modules`), the model used the tool and reported the
right files and lines without the decoy. A separate live research task (search the web, then fetch the official announcement) used `jarvis.web.search`
and `jarvis.web.fetch` together and summarised the real latest release with its source. Not done: search inside archives, binary formats or very large files.