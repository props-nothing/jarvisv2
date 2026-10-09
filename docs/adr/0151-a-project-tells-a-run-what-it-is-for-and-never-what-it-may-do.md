# ADR-0151: A project tells a run what it is for, and never what it may do

Status: Accepted
Date: 2026-10-10

## Context

JARVIS is used as an autonomous project manager: a B2B prospecting effort run by sub-agents on a six-hourly schedule, working for days with the owner mostly away. A chat answers a
question; this is something JARVIS **keeps working on**. Three things were missing, and the owner had been compensating by hand:

* every scheduled objective had to carry the whole brief (who the customers are, the tone, what not to do), pasted again each time;
* nothing said where the work lives, so each run re-discovered the folder;
* what one run decided was lost to the next unless the model happened to write a worklog file and the next run happened to find and re-read it.

## Decision

1. **A project is a goal, the owner's standing guidance, a working folder, a status and a journal.** Stored in three tables (`0017_projects.sql`): `projects` (name unique ignoring
   case, goal ≤ 4,000 characters, guidance ≤ 12,000, folder ≤ 512, status `active|paused|done`), `project_notes` (`progress|decision|blocker|next|result|owner`, ≤ 4,000 characters,
   the run that wrote it) and `project_links` (a session or a schedule belongs to a project). The folder is relative and may not contain `..`, a drive, a leading separator or a control
   character; it is a hint about where files live, and the file tools still resolve it against the granted roots.
2. **A run belongs to a project through its conversation.** `POST /api/v1/runs` accepts `project_id` (id or name) and binds the **session**; every later run in that session carries the
   project without repeating it. A schedule is linked the same way (`project_id` on create, `--project` in the CLI); a schedule made by the model from inside a project joins it; a
   sub-agent started from inside a project inherits it. A conversation that already belongs to another project is refused (409) rather than moved, and the check happens before anything is
   written, so a refusal leaves no run behind.
3. **The brief is policy, the journal is data.** The owner's goal, guidance and folder, plus how to run a project (read the journal, plan, delegate, record decisions, ask only when
   blocked), are a workspace-policy context item sent as a second system message. The journal (written partly by a model that may have read a hostile page) is a *derived* context item,
   fenced as untrusted data and sent as a user message, newest entries kept within the fence bound, one line per entry so a note cannot imitate another. A sub-agent gets the brief with
   sub-agent guidance and no journal.
4. **A project changes what a run is told, never what it may do.** No field carries a tool, scope or approval (`deny_unknown_fields`, and a test that tries), the policy, approvals,
   postures and granted folders are untouched, and the journal tool never takes a project argument: `jarvis.project.note` finds the project from the call, its run and that run's
   conversation, so a model cannot write into a project it is not working in. It records the run, cannot claim the owner's `owner` kind, asks no approval (one short line in the owner's own
   journal, reaching nothing outside; asking each time would bury approvals that matter, `ADR-0136`).
5. **Paused and done projects stop their schedules.** A fire for a project that is not active is claimed and counted as skipped, like an overlapping fire, so resuming does not release a
   backlog. The owner can still chat in a paused project; its brief says so.
6. **Surfaces.** `jarvis project add|list|show|set|pause|resume|done|note|remove`, `jarvis ask|chat --project NAME`, `jarvis schedule add --project NAME`; the console has a Projects panel on
   the Ops page (create, edit, status, journal, chat in it) and a project selector beside the conversation title.

## Consequences

The owner writes the brief once and edits it as the work teaches them. A recurring task wakes up already knowing the goal, the rules and what yesterday's run decided. Deleting a project
removes its journal and links; conversations, runs and schedules stay (a schedule simply loses its project).

Rejected: putting the project on the session row (session construction is strict and used everywhere, and a schedule needed the same link); letting the model choose a project per note (a
model could write into another project's journal); sending the journal as policy (it carries model-written text); a per-project permission set (it would be a second place for authority to
live, and the approval model deliberately has one).

Not built: per-project tool allowances, project-scoped memory, a project template library, and a journal view with search. Verified by tests at storage, route, scheduler, executor and tool
level, including the injection-shaped note staying out of the policy message. **Not yet verified** against a live model on a real project run (see `P9-055`).