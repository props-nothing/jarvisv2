# ADR-0153: JARVIS can create and change projects itself, and keep its memory straight, with the owner's yes

Status: Accepted
Date: 2026-10-10

## Context

The owner asked JARVIS, in conversation, to "create this as a project". JARVIS wrote a `PROJECT.md`, proposed a memory, and said the built-in journal "couldn't be used (this chat isn't attached to one)". Projects
(`ADR-0151`) could be made only by the owner, from the console or a terminal; the model had a journal tool that refused outside a project and no way to get into one. The same gap existed for memory (it could propose a
claim but never repair a wrong one) and for schedules (it could add and remove, not pause).

## Decision

1. **Project tools** (`project_tool.rs`): `jarvis.project.list`, `use`, `create`, `update` and `assign_schedule` join `note`. `create` makes the project and, when the conversation is in none, makes the conversation belong to it, so
   the journal works at once; `use` joins an existing project; `update` changes goal, guidance, folder, name or status; `assign_schedule` files a schedule under a project or takes it out. The conversation is always taken from
   the call (never an argument), and a conversation already in a project is not moved.
2. **What asks.** `create`, `update` and `assign_schedule` ask: a project's guidance is a standing instruction every later run, attended or not, is given as policy, so a page a model read must not plant one silently. `list`, `use` and
   `note` do not: they read, or only choose which existing project a conversation is told about. A project still never widens what a run may do.
3. **A run in no project is told which projects exist.** When at least one project is not done, a plain conversation receives a short index (name, status, goal, newest first, at most eight) as fenced data, with the two verbs to use.
   Without projects nothing is added, so ordinary chats cost nothing. Sub-agents are not given it.
4. **Memory: `jarvis.memory.correct` and `jarvis.memory.forget`** (`memory_manage.rs`), both asking. The owner's card shows the call's arguments, and a memory id means nothing to a person, so each call carries a **quotation of the claim**;
   after the yes, the tool checks the quotation is really in the stored claim (case and spacing ignored, at least eight characters unless it is the whole claim) and changes nothing when it is not, so a model cannot show one claim and
   change another. Writes go through the same `MemoryService` as the HTTP surface (version-guarded; a forget writes the tombstone). `jarvis.memory.search` now prints each hit's `memory_id` outside the fence so the model can name it.
5. **Schedules: `jarvis.schedule.pause`** (no ask: it only stops work) **and `jarvis.schedule.resume`** (asks: it starts unattended runs).

## Consequences

Asked to make a project, JARVIS now makes one, shown to the owner as an ordinary approval card. Tests: create-then-journal, no move between projects, join and list, update and schedule filing, the asking policy of every tool, the
index reaching a run in no project (and the brief still absent), pause and resume, memory correct and forget with a true and a false quotation, and the short-quotation rule.

Not built: a model-facing way to *confirm* a proposed memory (kept an owner act, `ADR-0140`), deleting a project from a tool (the owner's console and CLI only), and per-project tool allowances.