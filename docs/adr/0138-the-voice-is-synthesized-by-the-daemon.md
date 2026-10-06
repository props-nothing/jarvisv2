# ADR-0138: The voice is synthesized by the daemon

Status: Accepted
Date: 2026-10-06

## Context

The console spoke with the browser's own `speechSynthesis`: whichever voice the operating system ships (on Windows
usually a legacy robotic one), chosen before the browser had finished loading its voices, with the pitch lowered by us.
It sounds poor, and a product whose promise is a voice cannot rest on that. A hosted neural voice (ElevenLabs) needs an
API key, and the provider says plainly that such a key must not be in client-side code.

## Decision

1. **The daemon synthesizes speech; the page plays it.** `crates/jarvis-voice` holds a provider adapter (ElevenLabs'
   `text-to-speech/{voice}/stream`), and the daemon exposes `GET /api/v1/speech` (is a voice configured, which one; never
   the key) and `POST /api/v1/speech` (`{ "text" }` -> `audio/mpeg`) behind the same bearer credential as the rest of the
   API. The key is read from a file named by `daemon.speech_api_key_ref` (an absolute path, like the model key) at startup
   and held in a redacting wrapper: it is in no config value, log line, error or response.
2. **Opt in, with a good fallback.** With no key file the page uses the browser voice, now chosen well (a "Natural" or
   "Online" voice first, then Google, then legacy; British male preferred; voices re-read as they load; no pitch hack) and
   it says so when only a basic voice exists. If the neural request fails mid-answer, the rest is spoken with the browser
   voice and the page says why.
3. **Setup without typing a key into a shell.** `jarvis init --elevenlabs` takes the key from `ELEVENLABS_API_KEY` into a
   private file; `--voice-key-file PATH` uses an existing file; `--voice-id` and `--voice-model` choose the voice. There is
   no flag that takes the key itself.
4. **Natural pacing, streamed (amended 2026-10-06).** The first version collected the provider's whole response in the
   daemon and spoke only after the whole answer was written, which made the voice start seconds after the text, and it held
   the face in its "speaking" state from the moment audio was *requested*. Now: the daemon passes the provider's `/stream`
   chunks straight through (the first chunk is awaited so a refusal is still a real status code); the page speaks each
   sentence as soon as it is complete, while the answer is still being written, plays the audio progressively (Media Source,
   falling back to a whole blob) with the next two pieces already loading, and sends the sentence before as `previous_text`
   so the voice keeps its intonation (omitted for the `eleven_v3` models, which refuse it). The face is "speaking" only while
   sound is playing; while it waits for the voice it is "working" and the caption reads "preparing voice". A new message,
   Escape, the microphone or a click on the head aborts the in-flight requests and stops the audio at once. Measured on a real
   account: about 0.4 s from a finished first sentence to sound with `eleven_v4_turbo`.
   **Model choice:** `eleven_v3` is the provider's quality model and not a real-time one (about 2.4 times slower here), so the
   default stays `eleven_v4_turbo`; `eleven_flash_v2_5` is faster still.
5. **The head follows the real audio.** The page reads the level of the audio that is playing and drives the jaw from it
   (a synthetic rhythm only for the browser voice).
6. **Content policy.** Audio plays from a `blob:` the page builds from the daemon's own response, so the policy changes
   from `media-src 'none'` to `media-src blob:`; nothing else about it loosens.
7. **Bounds.** 1,500 characters per request, 8 MiB per response, a 25 s timeout, no redirects, no retries (a failed part
   falls back to the browser voice rather than repeating a paid call).

## Consequences

- Text the assistant is about to say leaves the machine for the provider when a key is configured; the setup output and
  the getting-started guide say so. Without a key nothing changes.
- Cost is per character; there is no cache and no limit beyond the per-request caps yet.
- Verified against a live account on 2026-10-06 (see `docs/research/integrations/elevenlabs.md`): latency by model, the
  `previous_text` refusal on `eleven_v3`, and end-to-end streamed playback in the browser. Not done: the WebSocket
  input-streaming endpoint (one voice context for a whole answer), regional endpoints.
- Speech to text is still the browser's recognizer (`P8-010`); a provider-backed recognizer and the Custom LLM voice path
  (`P8-003`) are separate slices.