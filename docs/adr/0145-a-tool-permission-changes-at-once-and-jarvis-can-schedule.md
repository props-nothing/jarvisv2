# ADR-0145: A tool's permission changes at once, and JARVIS can schedule work from a conversation

Status: Accepted
Date: 2026-10-09

## Context

Two gaps showed in daily use. Trusting a tool ("run without asking") needed a restart, so the approval card could not offer it:
the person who had just approved `npm run build` for the fifth time had to leave the conversation, open Settings and restart.
And the scheduler, which runs tasks while the owner is away, was reachable only as a command (`jarvis schedule`), so "remind me
tomorrow at nine" could not be said.

## Decision

1. **The permission policy is replaceable at run time.** `ToolPipeline` holds the workspace policy behind a lock as a snapshot;
   every call reads one snapshot and decides with it. Only the settings handler replaces it, with a policy built from the
   **saved** configuration by the same function the daemon uses at start (`compose_workspace_policy`), so there is one rule for
   what a policy is. The reply says `applied: true`, or `restart_to_apply: true` when the saved file could not be read or
   composed; in that case the running policy is left exactly as it was (a half-understood file never loosens anything). Both
   directions apply at once: turning a tool **off** stops it at the next call, not at the next restart. A call already
   decided keeps the snapshot it was decided with.
2. **The approval card offers "Always allow".** One click trusts that tool (the same `policy.trust` entry Settings, Permissions
   writes) and approves the call. It is the owner's own click on a labelled button, nothing is added to answering an approval
   (`ADR-0136`), and it can be undone in Settings. Anything that talks to other people still asks whatever is chosen (`ADR-0133`).
3. **Scheduling tools.** `jarvis.schedule.add` (an interval `every` such as `6h`, or one UTC time `at`), `jarvis.schedule.list`
   and `jarvis.schedule.remove` are native tools over the existing scheduler, using its own validation (interval bounds,
   future-only times, the per-workspace cap), so there is one set of rules. A scheduled task is an ordinary run started later: the
   same executor, policy and approvals. What is new is **persistence**: a standing instruction outlives the conversation, so a
   page a model read could try to leave one behind. Adding and removing therefore **ask the owner** (risk 2, `Ask`) and listing is
   free; "Always allow" on the card is the way to ask less. A listed objective is fenced as data. The model works out `at` from
   the clock it is given each run (`P9-035`).

## Consequences

Verified live: a reminder scheduled from chat and listed back; trusting the tool through the API applied at once (the next
schedule did not ask); turning it off was refused by policy on the very next call. Not done: a schedule that fires while the
console is closed still speaks nothing (results are read in `jarvis runs`); no edit of an existing schedule (remove and add).