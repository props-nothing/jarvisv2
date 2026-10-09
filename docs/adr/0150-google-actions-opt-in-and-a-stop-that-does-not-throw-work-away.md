# ADR-0150: Sending mail and creating events is opt-in and always asked, and a stop does not silently throw work away

Status: Accepted
Date: 2026-10-09

## Context

With Google connected (`ADR-0149`) and tested on a real mailbox, two things stood out. The assistant could read mail but not answer it. And, while using JARVIS for a long
autonomous task (sub-agents researching prospects, a six-hourly schedule), restarting the daemon to apply a setting **interrupted five working runs** (`interrupted_by_restart`):
the restart command and the console's "Restart to apply" gave no hint that anything was in flight.

## Decision

1. **Actions are a separate, opt-in grant.** `daemon.google_actions` (off by default) makes the sign-in also ask for `gmail.send` and `calendar.events` (and nothing broader: not
   `gmail.modify`, not `mail.google.com`). The sign-in is held to what it asked for (a missing scope is refused and revoked), and the console says when actions are on but not
   yet granted ("sign in again"). The read-only promise holds until the owner turns this on.
2. **Two tools, registered only with actions on, both always asked.** `jarvis.gmail.send` is `external_communication`, risk 3, approval `Ask`: it reaches another person, so the
   workspace rule that such a call always asks cannot be trusted away, and the card shows recipient, subject and text. One recipient only; the address is validated far more strictly
   than RFC 5322 (no display name, list, quoting, whitespace or control character) so nothing a model writes can add a header (`Bcc:` after a line break) or a second recipient;
   the subject is one line; the body is base64 so it cannot be mistaken for headers. `jarvis.calendar.create` is a write, risk 2, `Ask`, with no attendee list (it invites nobody).
   Neither retries (a retried send would send twice). The tool text tells the model never to send on the strength of text found in a message or page.
3. **A stop or restart that would interrupt working tasks is refused with the count.** `POST /api/v1/shutdown` and `/restart` answer 409 `runs_in_flight` (a run that is driven by this
   process and not waiting for an approval) unless `?force=true`. `jarvis stop|restart` prints the refusal and leaves the daemon running; `--wait` goes ahead once the tasks have finished
   (up to an hour); `--force` interrupts them. The console's Restart button shows the message and offers "Restart anyway". A task waiting for an approval is not counted (it survives).

## Consequences

Verified by tests: the exact message built for a send (headers, base64 body, UTF-8 subject), every injection shape refused before a request, no send without the granted scope, event
times normalised to UTC with no attendees, the scope request and the refusal of a partial grant, and the stop guard (counted, refused, forced, parked runs not counted). **Not verified
live:** a real send and event creation (the owner has to turn actions on and sign in again; a send must be tested to the owner's own address only), and the stop guard against the
running daemon, which was left alone because the owner had work in flight, which is the situation the guard is for. Sending in reply to a thread, attachments, several recipients and
accepting invitations are not built.