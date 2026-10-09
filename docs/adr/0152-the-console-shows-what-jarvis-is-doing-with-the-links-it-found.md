# ADR-0152: The console shows what JARVIS is doing as it happens, with the links it found

Status: Accepted
Date: 2026-10-10

## Context

A long task gave the owner one line of text ("step 4 · thinking · 40s") and a face that turned faster. Research is the case that suffers most: JARVIS searches, opens pages, reads them and
delegates, and the owner could not see where it looked, which pages it trusted, or what a sub-agent or a scheduled task was doing at all. The owner asked for the work to be *seen*: live
actions, relevant links, and animation on the face while it works.

## Decision

1. **The run's own event stream carries it; no new channel.** The tool request already announced itself; its payload now also names its **target** for the web and memory tools (`jarvis.web.fetch`:
   `host/path` with no query, fragment or credentials, because that is where a secret would be; `jarvis.web.search` and `jarvis.memory.search`: the query; `jarvis.agent.delegate`: the task, bounded). After
   a call runs, one more `activity_updated` event (`phase: tool_result`) says whether it worked and, for a search or a fetch, the **links** it touched (at most eight). A new event kind was rejected: the
   kind is a closed set with a storage check, and the activity event already means "a concise operational fact changed". Failing to record the event never fails the run.
2. **Links are untrusted and fail closed.** A link comes from a search service or a page, so it is offered only when it is a plain `http` or `https` address with a host, no embedded credentials and at most 300
   characters (`javascript:`, `data:`, `file:`, relative references and `user:pass@` are refused, with tests). The daemon checks it, and the console checks it again before rendering; it opens in a new tab with
   `rel="noopener noreferrer"`. Titles and targets are shown with `textContent` only. The model is told nothing new: the same text it already received is read back (`jarvis_web::links_from_results` parses the format
   `render` writes, beside it, with a test that a snippet cannot forge an entry).
3. **The console follows every working run, not just the chat's.** A scheduled task or a sub-agent has no conversation on the page, but its run has a stream; the console follows up to three (none for a run waiting for an
   answer, whose stream would sit open), so their actions appear with a `scheduled` or `sub-agent` tag.
4. **What it shows** (`mission.js`, a second static asset beside `hud.js` and `head.js`): a live feed (newest first: kind icon, what, about what, how long, spinner turning to a tick or cross, the links found), a sources
   strip, and animation around the face. The face makes room for the feed and glides right. Each action is a satellite on the outer ring with a beam to the core and packets flowing in (reading) or out (writing); each kind
   of work has its own effect: a radar sweep and rings for a search, a scan band for a page or file, contracting rings for memory, sparks for writing and commands, a twin orb for a sub-agent, and a net of thoughts
   while the model is reasoning. Completion bursts green or red. With reduced motion requested the motion effects are off; the feed stays.
5. **The Ops page and the rest of the console were revisited with it**: working cards show what each run is doing this second; scheduled tasks can be paused, resumed and removed from the page and show their project; recent
   answers expand in place; tools are grouped by family on the Ops page and in Settings (permissions) with plain labels for the settings; the long sheets scroll with their footer fixed; links in answers are clickable under
   the same rule; finished answers list their sources; an empty chat offers starting points.

## Consequences

The new payload fields are additive and bounded (well inside the 64 KiB event limit). Tests: target shaping and secret stripping, link vetting (hostile schemes, credentials, length, count), result classification, the
executor emitting both events, the parser, and the page tests (no markup insertion, nothing loaded from elsewhere, `rel="noopener noreferrer"`). Exercised live on a scratch profile with a real model: a multi-search research run with
page reads, a CLI-started run with a sub-agent watched from the console, and the Ops and Settings pages.

Not built: a replay of finished runs from history, clickable satellites, per-link preview, and the resume-after-approval path does not emit the finished event (the run's end closes the row instead).
A related fix found while doing this: a search reply is often several megabytes (each hit carries its page text), so the 512 KiB bound made real searches fail; it is now 8 MiB (the model still receives only a few hundred characters per hit).