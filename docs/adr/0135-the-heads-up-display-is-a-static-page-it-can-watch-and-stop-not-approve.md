# ADR-0135: The heads-up display is a static page the daemon serves, and it can watch and stop but not approve

Status: Accepted
Date: 2026-10-03

## Context

A real assistant shows its state and its work where a person can glance at it. `jarvis watch` is a terminal view; the
product needs a browser-visible display (an orb that says idle, working, or waiting for you, and the work in progress).
Every API route requires a bearer credential, so a browser page has two problems: how it is served, and how it gets
the credential.

## Decision

1. **The daemon serves two static assets, `GET /hud` and `GET /hud.js`, without the credential.** They hold no data and
   no secret; every figure is fetched by the script from the authenticated API. The exemption is one function
   (`hud::is_public_asset`): exact paths, `GET` only. A sibling path, a `POST` to the same path, and the whole API still
   refuse an unauthenticated request (`the_display_assets_are_public_and_nothing_else_is`).
2. **`jarvis hud` opens `http://127.0.0.1:PORT/hud#token=…`.** A URL fragment is never sent to a server or written to a
   request log; the script moves it into the tab's session storage and removes it from the address bar at once (so a
   reload works and the tab closing discards it). The command prints the address without the credential.
3. **The page can watch and stop, not approve.** An approval decision needs the one-time code delivered to a private
   file in the profile's state directory (`ADR-0018`); a browser cannot read it, and the display does not add a route
   that hands it out. It shows the exact command with a copy button instead. Stop (one run, or everything) needs no such
   code and is offered.
4. **Content security.** Every response carries `default-src 'none'; script-src 'self'; style-src 'unsafe-inline';
   connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`, `no-store`, `nosniff` and
   `no-referrer`. The script inserts daemon text with `textContent` only (a run's objective and answer are model
   output); a test refuses markup insertion, inline script, and a query-string credential.

## Consequences

- Visual presence exists in the browser with no dependencies, no build step and no new crate.
- The credential is a local, same-user, loopback credential held in a tab's session storage for the life of the tab, and
  it is in the browser's history for the one navigation that carried the fragment; a shared or untrusted browser
  profile is out of scope, as it is for any local credential file.
- Approving from the display needs a trusted desktop channel that can read the delivered code (the Tauri client,
  `P9-001`), or a spoken challenge (`P8`); it is deliberately not solved by weakening the code's delivery.

## Amendment (2026-10-03, `P3-036`): the display became a console, and voice began in the browser

The display is now a full console, not only a status page: a large animated orb (idle, working, listening, speaking,
waiting for you) whose waveform follows the microphone or the spoken answer; a streaming conversation (it follows the
run's server-sent events, shows tool use as chips, renders answers as safe DOM, and keeps the session across turns);
and the same four lists with Stop buttons. A third static asset, `/hud.css`, joined the page and script; the policy is
now `style-src 'self'` (no inline styles), `media-src 'none'`, and a `Permissions-Policy` that allows the microphone and
nothing else.

**Voice, first slice (`P8-010`, browser-native).** The page uses the browser's own speech recognition and synthesis:
push-to-talk (the mic button or `M`), an optional wake word ("Jarvis, ..."), spoken answers, and interruption (the
microphone, Escape, the orb or a new message silences it at once; wake-word listening pauses while JARVIS speaks so it
does not hear itself). **Hands-free control**: "Jarvis, stop" cancels everything running and "Jarvis, be quiet" silences
speech. Nothing in the page records or uploads audio (`MediaRecorder` is refused by a test, as is any outside origin).
In Chrome and Edge the browser's recognizer may send audio to its vendor's cloud service; the page says so on the mic
button, and local recognition is a later slice. Spoken **approval** is deliberately not offered: voice is a weak channel
(`security.md`), and the decision code stays in the private file.