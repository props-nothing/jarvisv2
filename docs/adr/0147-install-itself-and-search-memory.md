# ADR-0147: JARVIS installs itself, and the model can search its memory

Status: Accepted
Date: 2026-10-09

## Context

With one executable (`ADR-0144`), installing is copying a file somewhere permanent and telling the system where it is, and the pieces already
existed (`path install`, `service install`) but a person had to know to run both from the right folder. Separately, a run starts with the 24 most
relevant memories chosen before the model has said what it needs, so anything older or less obviously relevant was out of reach however long
JARVIS had been used.

## Decision

1. **`jarvis install [--service] [--dir DIR] [--no-path]`** copies the running program to a per-user folder (`%LOCALAPPDATA%\Programs\JARVIS`,
   `~/.local/share/jarvis` elsewhere), then runs the installed copy's `path install` and, with `--service`, `service install`. No administrator
   rights, nothing outside the user's folders. The copy is written beside the target and renamed over it, so an interrupted install never leaves
   half a program; a running copy (Windows refuses to replace it) is reported with "run `jarvis stop` first". **`jarvis uninstall`** stops it, removes
   the login service and the path entry and deletes the program, and says that settings, keys, memory and history were left. A relative `--dir` is refused.
   An MSI, a signed `.pkg` or a Tauri shell (`P9-019`) remain the way to a first-class installer; this is the step that needs none.
2. **`jarvis.memory.search`**: read-only, risk 0, no approval. Every query word must appear in a claim (case-insensitive), ranked by importance, at
   most 10, each result fenced as data with the same introduction a context carries. Each candidate passes the **same gate** the context assembler uses
   (`RetrievedMemory::new` with the same allowed types), so nothing is returned that a run would not have been given; there is no second, looser reader of
   memory. It reports how many claims it looked at, so an empty answer is not mistaken for an empty memory. There is no fuzzy match: no embeddings are
   written yet (`P4-005`).
3. The console's Settings, Brain tab shows the search key row (save, remove) so the key from `ADR-0146` is not command-line only.

## Consequences

Verified live: the program installed into a scratch folder from a downloaded-style copy, replaced itself, reported "already installed" when run from
there, and uninstalled; the model found a remembered fact through the tool; the Settings tab shows the search key. Not verified: `install` on macOS and
Linux (the copy and permissions are unit-tested; the path link is the existing `path install`), and `--service` was not run here (it is the existing
`service install`).