# ADR-0141: The console is a face with a conversation; operations are their own page

Status: Accepted
Date: 2026-10-06

## Context

The console showed everything at once: a three-column dashboard with the face as a small ornament in the middle, controlled by the
pointer. The face is what makes JARVIS read as a presence rather than a chat box with panels, and a face that follows the mouse is
a cursor, not a character.

## Decision

1. **The face is the page.** It fills the stage. It is not steered by the pointer. A small mind in `head.js` gives it its own life:
   an expression chosen from a set (neutral, pleasant, attentive, curious, amused, pleased, thinking, focused, alert, surprised,
   concerned, sleepy) that changes by itself while standing by, eye movement in quick jumps with the head following, irregular
   and occasionally double blinks, slow wandering of the head, breathing, an occasional sigh, and dozing off after a long quiet.
2. **It reacts to what is happening.** The state decides the base: listening is attentive, working is thinking or focused, speaking
   holds the mood of the answer (worry for a failure or apology, pleasure for good news, curiosity for a question) with brow lifts
   and nods on the beats of the voice, waiting for you alternates alert and curious, offline is asleep. The console reports events
   (`emote`, `nod`, `shake`): a nod and a smile when you approve, a shake of the head when you deny or stop, concern when a run fails.
   The console tells the face what happened; the face decides how to look, so no page code positions it.
3. **The conversation is a panel on the right.** Messages and the box stay together, with the speak and wake-word switches.
4. **Everything else moves to the Operations page** (`Ops` in the header, or `O`): what is running, scheduled and recent, the
   telemetry and the tool list. Settings stay their own screen.
5. **Nothing that needs you is hidden.** Approvals and "remember?" cards appear as a tray at the bottom left of the face page, with the
   same buttons, and the Ops button carries a count. Hands-free answering (a spoken yes or no) is unchanged.

## Consequences

- The head rig gained expression channels: eyes squash and stretch about their centres (blinks, squints, wide eyes), the corners of
  the mouth lift and drop (smile and frown), and the inner and outer ends of each brow move independently (worry, anger, a raised
  eyebrow), all weighted from the mesh geometry like the jaw and brow already were.
- A visitor who wants the dashboard presses `O`. Anything that could not wait for them is still on the face page.
- Reduced-motion users get a still face: the head does not wander, though the expressions still change.

## Falsification

Checked live: the face changed expression on its own while idle; the approval tray appeared and Approve and Deny worked from it with a
visible acknowledgement; the Ops page held every panel the old layout had; `O` and `Esc` toggled it.