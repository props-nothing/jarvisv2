# ADR-0132: A schedule is a request to start an ordinary run, fired at most once and never piled up

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P6-011`

## Context

Everything JARVIS could do so far happened because a person asked, right then. The difference between a chatbot and
an assistant is the other half: "check this every morning", "remind me at 4", "keep an eye on that page". Phase 6
(`ROADMAP.md`) is the full answer — a durable event bus, timezone-aware schedules, workflow state machines. That is
months of work, and a first useful slice does not need most of it.

The risks are specific. An unattended task that may fetch, compute or write is an agent acting with nobody watching.
A scheduler that fires twice sends two things; one that replays a week of missed fires floods a machine on waking; one
that queues a fresh approval request every minute buries the person who has to answer them.

## Decision

1. **A schedule is a request to start an ordinary run, and nothing more.** When one fires, the scheduler calls
   `GatewayState::start_and_drive` — the same function the REST route uses. A scheduled run therefore has exactly the
   authority an interactive run has: a tool the policy holds for a person is held for a person, and the run parks until
   they decide. The request carries an objective and a cadence and **no tool, scope or approval**
   (`deny_unknown_fields` makes an attempt a `422`). The scheduler never approves anything and cannot be asked to.
2. **At most once.** A fire is *claimed* — the next time advanced — in a guarded `UPDATE`
   (`WHERE id = ? AND next_run_nanos = <what it read>`) **before** the run starts. Two passes cannot both fire a task,
   and a crash between claim and start loses one fire and never repeats one. For a task that may send or compute, a
   missed briefing is an annoyance and a doubled action is an incident.
3. **A backlog is skipped, not replayed.** The next fire is `now + interval`, never `due + interval`. A laptop that
   slept through ten hourly fires owes one.
4. **A fire is skipped while the previous run is still going.** Typically that run is parked on an approval nobody has
   answered; a second identical request behind it helps no one. The skip is counted and shown in
   `jarvis schedule list`, so the person returns to *one* request to decide, not forty.
5. **A recurring task has a conversation.** Its runs share one session, so "what changed since yesterday" has an answer.
6. **The model is told nobody is there.** The objective is prefixed with a line saying the user is away and to report
   briefly instead of asking; the run list strips it back off for display (`[scheduled] <task>`).
7. **Two cadences only: `every N` and `once at <UTC instant>`.** "Every day at 08:00" needs a time zone and
   daylight-saving rules (`P6-003`); a half-right version fires an hour late for half the year without telling anyone.
   `--every 24h` is the honest form until `P6-003` lands. Intervals are 60 seconds to 366 days; a workspace holds at
   most 50 tasks; one pass fires at most 20.
8. **The scheduler shares the HTTP transport's lifetime** and runs only when the daemon drives runs: it is aborted
   before the listener drains, and a daemon with no executor has nothing to start runs for.
9. **`GET /api/v1/runs` lists recent runs with their answers.** A scheduled run has nobody watching its stream, so this
   is how its result is found (`jarvis runs`).

## Consequences

- JARVIS now works while you are away. Verified live against a real model: two tasks every minute; the arithmetic one
  ran each time ("6 times 7 is 42"); the fetch one parked on its first fire, was **skipped** on the second, and left
  exactly one approval pending.
- An unattended task that needs a person waits, indefinitely. That is the safe behaviour and also the visible cost:
  `jarvis ask` and `jarvis runs` surface it, but there is no push notification yet (`P6-007`).
- A task that fires while the daemon is down fires once, late, after it returns.
- Nothing yet triggers a run from an *event* (an email arriving, a file changing); that needs the event inbox
  (`P6-001`/`P6-002`).
- SQLite only, like the rest of the control-plane tables.

## Falsification

`a_due_task_is_claimed_by_exactly_one_pass` runs two claims over one due task; `a_long_absence_owes_one_fire_and_the_next_is_measured_from_waking`
sleeps a task for ten hours; `a_fire_is_skipped_while_the_previous_run_is_still_going` leaves a run unfinished and asserts
the second fire starts nothing and is counted; `a_recurring_task_continues_one_session` asserts the session is reused; and
`a_bad_schedule_is_refused_with_the_rule_it_broke` asserts a schedule cannot carry an approval.
