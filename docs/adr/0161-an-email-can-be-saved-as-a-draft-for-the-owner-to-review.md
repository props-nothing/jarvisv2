# ADR-0161: An email can be saved as a draft for the owner to review

Status: Accepted
Date: 2026-10-10

## Context

`jarvis.gmail.send` always asks the owner, and rightly: it reaches another person. But the owner often wants to *see* the email first, edit a word, and press send themselves, without approving a card in the console. A draft in Gmail is exactly that, and with attachments and reply threading (`ADR-0157`) a run can prepare a complete message.

## Decision

1. **`jarvis.gmail.draft`** takes the arguments of `jarvis.gmail.send` (one recipient, subject, body, up to five attachments from a granted folder, `reply_to_message_id`) and saves the message in the owner's Gmail Drafts through `drafts.create`. It sends nothing. It uses the same message builder and the same checks as a send, including the do-not-contact list.
2. **It runs without asking** (effect `write`, risk 1): the draft stays in the owner's own mailbox and the owner is the one who reads it and presses send. The model is told to prefer a draft when the owner has not said exactly what to send or the message matters.
3. **A new permission.** Google's `drafts.create` does not accept `gmail.send`; it needs `gmail.compose` ("manage drafts and send emails", a restricted scope; `docs/research/integrations/google.md`, Finding 26). It is asked for with the other action scopes when Google actions are on, so an owner who already signed in must **sign in again**; until then the tool says so and Google is not called. The console's "not granted yet: sign in again" hint now includes it.

## Consequences

JARVIS can prepare mail the owner finishes. The cost is one more permission (restricted, so the owner's Google Cloud consent screen must allow it) that also covers sending, which `gmail.send` already did. Not built: listing, editing or sending drafts through JARVIS, and the live check of the consent flow.