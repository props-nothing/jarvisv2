# ADR-0154: The face stands alone, and what JARVIS did stays on screen as a constellation

Status: Accepted
Date: 2026-10-10

## Context

The main page drew a film-style frame around the face: concentric rings, a ring of the system's name, a voice ring, ticks, corner brackets, status text, and a neck and skull in wire. The live view (`ADR-0152`) rode that
ring, so each action appeared as a satellite and was gone five seconds after it finished: a task with several quick calls left nothing to look at. The owner asked for the face alone, smaller, and for the live
view to stop disappearing.

## Decision

1. **Face only.** All ring, tick, arc, bracket and text drawing is removed from the canvas, and the wire skull and neck from `head.js`. What remains is the mesh, a wide faint glow, a few drifting motes (faster when busy), and
   a thin line of voice bars under the face while someone is talking. The face is about half its former size, and the state caption sits just under it wherever it is.
2. **A constellation instead of a ring** (`mission.js`). Each action is a node on a soft elliptical orbit round the face. A running action is on the inner orbit, bright, labelled, with a beam and packets to the face; when it ends it
   stays bright for 2.6 seconds (so a quick call is still seen), then settles and drifts outward one orbit at a time as newer actions arrive (up to 26 nodes). Links appear as small moons round the node. Nodes are kept while the run
   goes on and for 30 seconds after it ends, and are put away when a new request begins. A label never turns back across the face: it is cut to the room outside.
3. **From the picture to the detail.** Pointing at a node names it; pressing it opens its step in the feed, scrolls to it and flashes it. Pressing a step opens or closes it and pulses its node.
4. **The feed keeps the whole run.** Up to 40 steps, newest first, in a scrolling column; the newest four are open (target and links), older ones are single lines until pressed. The sources strip keeps every link.
   After 90 seconds without activity the panel folds to one line ("DONE · 5 steps · 13 sources") and the face returns to the centre; pressing the line opens it. A × puts it away until JARVIS does something new.
5. **Unchanged:** the effects per kind of work (radar, scan, contracting rings, sparks, twin orb, thought net), the safety rules for links, and reduced-motion behaviour.

## Consequences

The page is quieter at rest and shows more while working. Checked live (scratch profile, real model): a research run with searches and page reads leaves its five nodes and steps on screen, hover and press work, the panel folds and the
face glides back. The page tests still hold (no markup insertion, nothing loaded from elsewhere). Not built: replay of a past run's constellation from history, and dragging nodes.