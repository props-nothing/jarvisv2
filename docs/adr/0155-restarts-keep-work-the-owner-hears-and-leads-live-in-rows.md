# ADR-0155: A restart no longer drops work, the owner hears when it matters, and prospect data lives in rows

Status: Accepted
Date: 2026-10-12

## Context

A review of what JARVIS does as an autonomous project manager found gaps that make unattended work fragile or opaque:

- A run cut off by a restart or update was settled as failed and nothing continued it. The owner had asked for something and it silently stopped.
- A long project forgot what it had decided: the journal shown to a run is the newest few notes, so a decision made weeks ago fell out of view.
- A project could not be capped: a mis-scheduled task could start runs without limit.
- Nothing told the owner that an approval was waiting or that a scheduled task had finished unless the console was open, and there was no "what did it do while I was away".
- A prospecting project kept its leads in a CSV file that a run rewrote, so "who have we already contacted?" had no reliable answer.
- A security review of the new project and memory tools found one real issue: the approval card clipped long arguments, so a harmless long `goal` could hide a hostile `guidance` that every later run would be told as policy.

## Decision

1. **Some tools can never be trusted silently.** `jarvis.project.create|update|assign_schedule` and `jarvis.memory.correct|forget` change standing instructions or the owner's memory. They are listed in `NEVER_TRUSTED_TOOLS`; the policy ignores a
   `trust` entry for them and `PUT /settings/tools/{tool}` refuses `trusted` with a 422. The approval card shows every argument field whole, there is no "Always allow" button for them, and a spoken "yes" does not answer them (a "no" does).
   `jarvis watch` says when it cut an argument and points at `jarvis approvals`.
2. **The journal keeps what matters.** A run is shown the newest notes and, before them, up to 30 older decision, result, blocker and owner notes (1300 characters), oldest first.
3. **A project can cap its scheduled runs** (`daily_run_limit`, 0 to 1000, 0 meaning none; migration 0018). The scheduler counts the project's runs in the last 24 hours (a rolling window, so no time zone) and skips fires over it, as it already skips paused
   projects. A person's own message is never refused. The cap is the owner's dial: the model's `update` tool cannot set it.
4. **A run a restart cut off gets one continuation** (`daemon.resume_interrupted`, on by default). Recovery still settles it as failed, truthfully. Then, for runs that started in the last two hours and are the newest of their conversation, the daemon
   starts one new ordinary run in the same conversation whose objective begins with a notice to check what was already done before repeating it, least of all anything that sends or changes something outside the machine. At most three start; a continuation,
   a sub-agent, and a run that was cancelled are never continued. It is an ordinary run, so approvals, policy and audit apply unchanged.
5. **A digest.** `GET /api/v1/digest[/hours]`, `jarvis digest [hours]` and an Ops card: runs, outcomes, runs waiting for the owner, tokens (summed from each model call's usage event; shown as "not reported" when the provider sends none, as ollama cloud does today), each project's runs and newest decisions, results and blockers, and the newest failures. Read-only, over existing rows.
6. **Push through ntfy, off by default** (`daemon.push_topic`, `daemon.push_server`; `docs/research/integrations/ntfy.md`). When no console is open, a new approval and a finished scheduled task are announced on the desktop (as before) and, when a topic is set,
   pushed. The push is content-free (that an approval is waiting and for which tool, or that a task finished): never an answer, a question or an argument, because the topic is the only secret on a server the owner may not control. It is the daemon speaking to its
   owner, not a tool, so a model cannot cause it. `jarvis push test` sends a test message.
7. **Contacts as rows** (migration 0019, `jarvis.contacts.save|search|stats`, `jarvis contacts list|stats|add|remove|export`). Saving the same lead twice updates it (by address, else company and person). Statuses run `new` to `won`, `lost` or `do_not_contact`. The tools
   ask nothing, since they only write local rows, and a model reads notes and source back fenced as data. The one rule that matters is enforced in storage: a model cannot move a contact out of `do_not_contact`; only the owner can. A contact found inside a project is filed under it.
8. **Small console changes:** runs parked on an approval are followed as a held step in the constellation (working runs first, three in all, because a browser allows six connections to one host); the project sheet has a Daily runs field and shows runs used; CI parses the console scripts.
9. **Memory search ranks by words.** When no claim contains every word of the question, the claims holding the most of them are returned and the reply says it was a partial match.

## Consequences

Updates and crashes no longer silently drop work, a long project keeps its decisions, a runaway schedule is bounded, and the owner can look at one screen (or their phone) to know what happened. The cost is more surface: a continuation can repeat an effect the
interrupted run had already made, which is why the notice tells it to check first and why nothing outward-facing is trusted without an approval; the push sends a metadata-only message to a third party unless the owner runs their own server.

Not built, recorded in `TODO.md` `P9-069`: ntfy access tokens, enforcement of `do_not_contact` inside the Gmail send tool (today it is advice every run reads plus the owner's approval of each send), a contacts window in the console, and replay of past runs.
