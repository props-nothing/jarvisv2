# ADR-0156: A run waits out a provider rate limit, and the Gmail send tool honours do_not_contact

Status: Accepted
Date: 2026-10-10

## Context

Ten of the last 134 runs on the owner's machine failed with `model_rate_limited`, including scheduled and sub-agent runs nobody was watching. The model adapter retries a rate-limited call three times with a 0.5 to 1 second backoff
(or the provider's `Retry-After`), which is enough for a blip but not for a provider limit that lasts a minute. An unattended task that fails on the first such limit loses its whole turn and waits for the next schedule.

## Decision

When a model call is refused with a rate limit **before any stream opened**, the run waits and asks again: up to four more tries, after 20, 45, 90 and 180 seconds. Each wait is recorded as an activity event
(`rate_limited`, with the seconds) so a person watching sees why it is quiet, and the wait is taken a second at a time so a cancellation ends it at once and settles the run as cancelled.

Only a call that never opened a stream is asked again. A failure part-way through a stream is not, because some of the answer may already have been said and asking again could say it twice. After the last try the run fails with
`model_rate_limited` as before.

## The send tool checks the contact list

`jarvis.gmail.send` now looks the recipient up in the contact list (`ADR-0155`) before it sends, ignoring case. An address marked `do_not_contact` is refused with a reason, nothing reaches Google, and a contact list that cannot be read also refuses
(fail closed). This lives in the tool, not the prompt, so it holds for a run that ignores its guidance; the owner still approves each send as before, and only the owner can lift the status.

## Concurrent run starts no longer fail

The post-restart continuation exposed an older fault: starting runs at the same moment (a person, the scheduler and a continuation can all do it) sometimes failed one of them with "the local database is not available". `start_run` reads (it checks the
identity) and then writes inside a deferred SQLite transaction; if another writer committed in between, SQLite refuses the upgrade at once (`SQLITE_BUSY_SNAPSHOT`) and the 5-second busy timeout does not apply. The transactions that read before
writing (`start_run`, the memory purge and the summary write) now begin with `BEGIN IMMEDIATE`, which takes the write lock up front and waits for it. The database errors behind a refused run request are now logged (not shown to the client).
A test starts 24 runs at once and fails without the change.

## Consequences

A short provider limit no longer costs a task its turn; the worst case is a run that holds for about five and a half minutes before failing, which the console and `jarvis watch` already show as "working". Other transient failures are
unchanged (the adapter's own three tries). The send guard covers only the address in the list: a lead saved with no address, or a different address for the same person, is not matched. Not built: switching to a second model when one is limited (`P9-069`, model fallback).
