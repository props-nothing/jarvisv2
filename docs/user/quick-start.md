# Quick start

For someone who wants to use JARVIS, not build it. Contributors: see [development/getting-started.md](../development/getting-started.md).
The target experience, and what is still missing from it, is in [product/onboarding.md](../product/onboarding.md).

## What you need

- A model. The easiest is [Ollama](https://ollama.com) running on this machine (local models, or Ollama Cloud models through it).
  Any OpenAI-compatible server works too.
- Optionally Docker, if you want the assistant to run code in a throwaway container.
- Until releases are packaged (`P9-017`), a Rust toolchain to build it: `cargo build --release`. The two programs you need,
  `jarvis` and `jarvisd`, are in `target/release/`. **Keep them in the same folder.**

## First run

```text
jarvis
```

That is the whole thing. The first time, it asks which model to use (it lists what your Ollama has) and which folder, if any,
the assistant may read and write; then it starts in the background and opens the console in your browser, already signed in.
Later runs skip the questions, make sure it is running, and open the console. `jarvis --no-open` starts it without opening a
window.

Setting up without questions (scripts, other providers):

```text
jarvis init --base-url https://api.example.com/v1 --model NAME --api-key-file C:/path/to/key.txt --workspace C:/Users/me/notes
jarvis start
```

`--workspace` may be repeated. The assistant can read, list and create files in those folders; changing an existing file is
asked about first. Nothing outside them is reachable.

## Keys

| Key | What it is for | How to give it today |
| --- | --- | --- |
| Model key | A hosted model provider (not needed for local Ollama) | `jarvis keys set model --from-env NAME` (or `--file PATH`, or pipe it in), or `--api-key-file PATH` at `init`. |
| Voice key | A natural voice (ElevenLabs) | `jarvis keys set voice --from-env ELEVENLABS_API_KEY`, then `jarvis restart`. Without it the console speaks with your browser's own voice. |

```text
jarvis keys status               # which keys are set (never their values)
jarvis keys set voice --from-env ELEVENLABS_API_KEY
jarvis keys test model           # asks the model server to accept the key
jarvis keys test voice           # asks the running daemon to make a sentence of audio
jarvis keys remove voice
```

Keys are written to a private file and the configuration holds only its path; they are never printed, logged or sent to the
browser, and never taken as a command-line value (it would stay in your shell history). With no flag and a terminal, `keys set`
asks you to paste it (visible as you type; a hidden prompt is `P9-013`).

## Changing settings

```text
jarvis config show               # every setting you can change, and whether key files are present
jarvis config get executor_model_name
jarvis config set executor_model_name deepseek-v4-flash:cloud
jarvis config set tool_workspace_roots C:/Users/me/notes C:/Users/me/projects    # replaces the list
jarvis config set policy.trust jarvis.files.edit        # tools allowed to run without asking
jarvis config unset speech_model
jarvis restart                   # apply
```

Each change is checked by the same rules the daemon starts with and refused, leaving the file untouched, if it would not
start (for example a voice id without a voice key). Changing one setting changes only that one: your folders, trust list and
voice stay as they were. `jarvis config` is the one place to edit; a Settings screen in the console is next (`P9-015`).

## Using it

- **The console** (opened for you; `jarvis hud` opens it again): talk or type, watch what it is doing, answer anything it asks with
  one click, **Stop** a run or everything, and see which tools it can use and which it asks about first.
- **Voice:** the microphone button (or `M`) to talk; turn on **Speak answers**; enable the **wake word** and say "Jarvis, ...".
  "Jarvis, stop" cancels everything. Voice recognition is your browser's (Chrome or Edge).
- **The terminal:** `jarvis chat`, `jarvis ask "..."`, `jarvis watch`, `jarvis approvals`, `jarvis cancel --all`, `jarvis runs`,
  `jarvis schedule`, `jarvis memory`, `jarvis tools list`. Every command has `--json` where it prints data.
- **Approvals are a yes or no.** Anything it asks about appears with Approve and Deny; you can also say "yes" or "no", or use
  `jarvis approvals approve|deny`.

## Stopping and troubleshooting

- `jarvis stop` ends the background program gracefully; `jarvis restart` stops it (if it is running) and starts it again, for example
  after you change a setting. Both wait until it has really finished.
- `jarvis doctor` diagnoses the installation; `jarvis logs --lines 50` shows what the daemon said; `jarvis status` says whether it
  is running.
- "No credential": open the console with `jarvis hud`, not by typing the address.
- A portable, throwaway profile: add `--root DIR` (an existing folder) to any command.