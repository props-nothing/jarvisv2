# ADR-0157: Email can carry files, answer in its thread, and save what arrives

Status: Accepted
Date: 2026-10-10

## Context

`jarvis.gmail.send` sent plain text only. A prospecting or project run that needs to send an offer, a report or a quote could write the file but not attach it, could not answer a message inside its conversation (the owner saw a new thread each time), and could not
receive a file that someone sent: `jarvis.gmail.read` gave the text and said nothing about attachments.

## Decision

1. **Attachments on send.** `jarvis.gmail.send` takes `attachments`: up to five paths inside a folder the owner granted (5 MB each, 10 MB together). They are read through the granted folders' confinement handles, so a path that climbs out or follows a link out fails like it
   does for the file tools. A name that says it holds a secret (`.env`, keys, certificates, anything with "credential" or "secret" in it) is refused outright. The approval card already shows every argument, so each file's path is in front of the owner when they answer; every send still asks.
   The message is one `multipart/mixed` `raw` message (the documented way, `docs/research/integrations/google.md`, Finding 25); no new scope is needed.
2. **Replies.** `reply_to_message_id` makes the send a reply: the original's subject (with one `Re:`), `In-Reply-To` and `References` from its identifiers, and its `threadId`. The recipient is still given explicitly and checked against the contact list. The identifiers come from a message a stranger wrote, so
   only well-formed `<id@host>` tokens are kept and a file name is reduced to plain characters: a line break must not become a header.
3. **Reading and saving attachments.** `jarvis.gmail.read` lists each attachment (number, plain name, type, size). New `jarvis.gmail.save_attachment` saves one into a granted folder (default `email-attachments`), never replaces a file (`name (2).ext`), refuses programs by extension, and caps size at 5 MB. It is a write
   from a stranger''s content, so it asks by default (risk 2). It is offered only when a folder is granted; `jarvis.gmail.send` without a grant refuses an attachment in words that say how to fix it.
4. The MIME building and the name and header rules live in one module (`mail_mime.rs`); the file operations in `google_files.rs`.

## Consequences

JARVIS can send a quote and answer in the thread, and can take in the PDF a prospect sends back. The limits are conservative because Google''s own size limit for `messages.send` was not found in the pages read; a live send is still to be run (it would email a real address). A saved attachment
is untrusted content in the owner''s workspace: the tool says so, refuses programs, and does not run anything. Not built: attaching to a draft (needs the `gmail.compose` scope, a new consent), inline images, and reading an attachment''s text (PDF and spreadsheet reading remain `P9-069`).
