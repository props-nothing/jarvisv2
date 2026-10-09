# ADR-0149: Sign in with Google from Settings, and read mail and calendar

Status: Accepted
Date: 2026-10-09

## Context

The connector crate (`P5-001`, `P5-002`) holds the contracts and the OAuth protocol walk for Google, but nothing in the daemon called it, so a person
could not connect an account at all. The ask was to make it as simple as signing in with a Google account from the settings menu.

## Decision

1. **The owner brings an OAuth client; signing in is then one button.** A Desktop-app client belongs to a Google Cloud project and the Gmail scopes are
   restricted, so JARVIS cannot ship a shared client. Settings, Google takes the client id (`daemon.google_client_id`) and the client secret
   (`daemon.google_client_secret_ref`, written by `jarvis keys set google` or the console), explains the five-minute one-time setup (including **publishing**
   the app, because Testing mode ends a sign-in after 7 days), and shows **Sign in with Google**.
2. **The daemon does the OAuth.** `GoogleAccount` uses the connector crate's `AuthorizationTransaction` (PKCE `S256`, random `state`, single-use, loopback redirect
   on the daemon's own port) and makes the two HTTP calls the crate deliberately does not (code exchange, refresh). The refresh token is stored in a private file
   (`google.token`); the access token only in memory. A pending sign-in is superseded by a new one and expires after ten minutes.
3. **One public route.** Google's redirect cannot carry the bearer credential, so `GET /oauth/google/callback` is public, exactly that path and method. It is inert
   unless its `state` matches a sign-in started a moment ago through the authenticated route; the first answer consumes the transaction whether or not it was valid,
   an error from Google, a forged state, a wrong path, an expired transaction and a repeat are all refused and store nothing, and the page it returns uses fixed words
   and echoes nothing from the request (falsification tests for each).
4. **Read-only scopes, read-only tools, asked first.** `gmail.readonly` and `calendar.readonly` only; a grant missing either is refused and revoked. Tools
   `jarvis.gmail.search`, `jarvis.gmail.read` and `jarvis.calendar.events` are risk 2 (held for the owner by default; "Always allow" on the card), declare
   confidential output, and return everything fenced as untrusted data, because mail is written by strangers. Nothing sends, changes or deletes.
5. **Signing in never needs a restart; the tools do.** Credentials are read from the saved configuration at each use, so the sign-in works at once; the tools are
   registered at start when a client id is configured, and answer "Google is not connected" until the owner signs in. Disconnect revokes at Google and deletes the file.
6. Errors are a fixed set of sentences; Google's own error text, tokens, codes and the client secret appear in no log, reply or error.

## Consequences

Not verified here (needs the owner's own client and account): a real sign-in, the live token exchange, and the live Gmail and Calendar reads; what was verified is the
authorization URL (Google parsed it and refused only the fake client id), the whole flow against a local fixture, and the console tab. The redirect path
`/oauth/google/callback` differs from the connector crate's registered `/` (recorded in the research addendum). Mail text the model reads goes to whichever model the owner chose,
which may be a cloud model; the user guide says so. Sending mail, accepting invitations and push notifications (`users.watch`) are not built.