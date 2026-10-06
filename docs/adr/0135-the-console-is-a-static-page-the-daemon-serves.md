# ADR-0135: The console is a static page the daemon serves, and it is where you talk, watch, answer and stop

Status: Accepted
Date: 2026-10-03 (amended 2026-10-06)

## Context

A real assistant shows its state and its work where a person can glance at it, and lets them answer it without
touching a terminal. `jarvis watch` is a terminal view; the product needs a browser-visible console: a face that says
idle, working, listening, speaking, or waiting for you, a conversation, the work in progress, and buttons for the two
things a person does to work in progress: answer a question and stop it. Every API route requires a bearer
credential, so a browser page has two problems: how it is served, and how it gets the credential.

## Decision

1. **The daemon serves static assets without the credential: the page at `GET /` (alias `GET /hud`), `GET /hud.js`,
   `GET /head.js` and `GET /hud.css`.** They hold no data and no secret; every figure is fetched by the script from the authenticated
   API. The exemption is one function (`hud::is_public_asset`): exact paths, `GET` only. A sibling path, a `POST` to
   the same path, and the whole API still refuse an unauthenticated request
   (`the_display_assets_are_public_and_nothing_else_is`).
2. **`jarvis start` opens the console** (`--no-open` skips it), and **`jarvis hud`** opens `http://127.0.0.1:PORT/#token=…`.** A URL fragment is never sent to a server or written to a
   request log; the script moves it into the tab's session storage and removes it from the address bar at once (so a
   reload works and closing the tab discards it). The command prints the address without the credential.
3. **The console can answer.** A held call appears under "Waiting for you" with the tool, its risk and the exact
   arguments, and **Approve** / **Deny** buttons. It decides with the same authenticated route the CLI uses
   (`POST /api/v1/approvals/{id}/decision`, `resume: true`, `channel: "desktop"`). The owner is the holder of the local
   credential; there is nothing else to fetch or enter (`ADR-0136`).
4. **Voice.** The page uses the browser's own speech recognition and synthesis: push-to-talk (the mic button or `M`),
   an optional wake word ("Jarvis, ..."), spoken answers, and interruption (the microphone, Escape, the orb or a new
   message silences it at once; wake-word listening pauses while JARVIS speaks so it does not hear itself). When
   something new needs an answer and spoken answers are on, it says so ("I need your permission to run some code. Say
   yes to allow it, or no.") and listens; a bare **yes** or **no** then answers the one waiting question (with several
   waiting it asks you to use the buttons, so a word never decides the wrong one). "Jarvis, stop" cancels everything
   running and "Jarvis, be quiet" silences speech. Nothing in the page records or uploads audio (`MediaRecorder` is
   refused by a test, as is any outside origin). In Chrome and Edge the browser's recognizer may send audio to its
   vendor's cloud service; the mic button says so, and local recognition is a later slice (`P8-010`).
5. **The face.** A film-style interface drawn on a canvas: concentric rings of fine ticks, segmented arcs turning
   against each other (faster while working), the system's name laid round a ring so that it fills exactly one turn,
   three heavy partial arcs and a radial spectrum that is the voice (microphone level while listening, a synthetic
   envelope while speaking), framing **a head**: a real human face mesh (MediaPipe's canonical face model, 468
   vertices, Apache-2.0, taken from upstream and recorded in `docs/research/integrations/mediapipe-canonical-face-model.md`)
   drawn as a hologram in plain 2D canvas, with a wire skull and neck behind it. The jaw opens with speech, the eyes
   glow and follow the pointer, the brows lift when something needs you, the head leans in while listening, and a scan
   plane passes over it (faster while working). Colour and tempo carry the state: cyan idle/working,
   green listening, violet speaking, amber when something needs you, grey offline. The layout is three columns:
   waiting and working on the left, the face with the conversation and the command bar in the middle, telemetry
   (counts derived from the same lists), scheduled, **can do** and recent on the right, in framed panels. "Can do"
   lists every tool and whether it **runs** or **asks** first or is **off**, as decided by the policy now in force
   (`asks_first` on `GET /api/v1/tools`, pinned to `evaluate` by a test), so the owner can see the posture at a glance. Nothing on the screen is
   invented data: every number is a count of the lists the API returns. `prefers-reduced-motion` slows the rings.
6. **Content security.** Every response carries `default-src 'none'; script-src 'self'; style-src 'self'; connect-src
   'self'; media-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`, a `Permissions-Policy`
   that allows the microphone and nothing else, `no-store`, `nosniff` and `no-referrer`. The script inserts daemon text
   with `textContent` only (a run's objective and answer are model output); tests refuse markup insertion, inline
   script or style, an outside origin and a query-string credential.

## Consequences

- Visual presence, a conversation, voice and one-click or one-word answers exist in the browser with no dependencies,
  no build step and no new crate.
- The credential is a local, same-user, loopback credential held in a tab's session storage for the life of the tab,
  and it is in the browser's history for the one navigation that carried the fragment; a shared or untrusted browser
  profile is out of scope, as it is for any local credential file.
- A spoken yes or no is as good as the browser's recognition and the room around it. That is accepted for a
  single-owner local product; a deployment with several people will record and require who answered (`P10`).
- The microphone path could not be exercised in the build environment (no audio device).